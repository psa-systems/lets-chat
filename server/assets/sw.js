// lets-chat service worker.
//
// Responsibilities:
//   1. PWA shell cache: pre-cache a small set of static assets under a
//      versioned cache name; serve them offline; purge old versions on
//      activate. `__ASSET_VERSION__` is substituted server-side
//      (routes/push.rs) from the build's asset version, so a new deploy
//      gets a new cache name and the activate handler evicts the stale one.
//   2. Offline outbox: intercept POSTs to message-create endpoints; when
//      the network is unreachable, stash the request in IndexedDB and
//      return a synthetic success so the composer clears. Replay the queue
//      on reconnect (Background Sync where available, plus an explicit
//      drain message from the page on the `online` event).
//   3. push:              parse JSON payload, render OS notification.
//   4. notificationclick: focus or open the target tab.
//
// IMPORTANT: the push handler suppresses showNotification() when any
// visible client is already on data.target_path. This is *baked in*, not
// optional - the user is actively reading the room, an OS-level ping
// would be noise. If a future tweak wants to relax this, do so behind an
// explicit user setting.

const ASSET_VERSION = '__ASSET_VERSION__';
const CACHE_NAME = 'lc-shell-' + ASSET_VERSION;

// Versioned assets are requested by the page with `?v=<asset_version>`;
// pre-cache them under the same query so the keys match a page fetch.
// Unversioned entries (offline page, manifest) are requested as-is.
const PRECACHE_URLS = [
  '/assets/offline.html',
  '/assets/manifest.webmanifest',
  '/assets/favicon.svg',
  '/assets/icon-192.png',
  '/assets/icon-512.png',
  '/assets/tailwind-built.css?v=' + ASSET_VERSION,
  '/assets/main.css?v=' + ASSET_VERSION,
  '/assets/vendor/htmx.min.js?v=' + ASSET_VERSION,
  '/assets/vendor/htmx-ext-ws.js?v=' + ASSET_VERSION,
  '/assets/vendor/idiomorph.min.js?v=' + ASSET_VERSION,
];

const OFFLINE_URL = '/assets/offline.html';

// Message-create endpoints we queue when offline. DMs post to the same
// /room/{id}/messages endpoint (a DM is a room of type "dm"). Threads use
// the nested path.
const MSG_RE = /^\/room\/\d+\/(thread\/\d+\/)?messages$/;

self.addEventListener('install', (event) => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE_NAME);
    // Add one at a time and tolerate individual misses: a single 404 in
    // cache.addAll() would reject the whole install.
    await Promise.all(PRECACHE_URLS.map((u) => cache.add(u).catch(() => {})));
    await self.skipWaiting();
  })());
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    const keys = await caches.keys();
    await Promise.all(
      keys.filter((k) => k.startsWith('lc-shell-') && k !== CACHE_NAME).map((k) => caches.delete(k))
    );
    await self.clients.claim();
  })());
});

// ---- Fetch routing ---------------------------------------------------------

self.addEventListener('fetch', (event) => {
  const req = event.request;
  const url = new URL(req.url);

  if (req.method === 'POST' && url.origin === self.location.origin && MSG_RE.test(url.pathname)) {
    event.respondWith(handleMessagePost(event));
    return;
  }
  if (req.method !== 'GET' || url.origin !== self.location.origin) return;

  if (req.mode === 'navigate') {
    event.respondWith(networkFirstNav(req));
    return;
  }
  if (url.pathname.startsWith('/assets/')) {
    event.respondWith(cacheFirst(req));
  }
});

async function networkFirstNav(req) {
  try {
    return await fetch(req);
  } catch (e) {
    const cache = await caches.open(CACHE_NAME);
    return (await cache.match(req)) || (await cache.match(OFFLINE_URL)) || Response.error();
  }
}

async function cacheFirst(req) {
  const cache = await caches.open(CACHE_NAME);
  const hit = await cache.match(req);
  if (hit) return hit;
  try {
    const res = await fetch(req);
    if (res && res.ok) cache.put(req, res.clone());
    return res;
  } catch (e) {
    return Response.error();
  }
}

// ---- Offline outbox --------------------------------------------------------

const DB_NAME = 'lc-outbox';
const STORE = 'queue';

function idbOpen() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(DB_NAME, 1);
    r.onupgradeneeded = () => {
      r.result.createObjectStore(STORE, { keyPath: 'id', autoIncrement: true });
    };
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

function idbStore(db, mode) {
  return db.transaction(STORE, mode).objectStore(STORE);
}

async function idbAll() {
  const db = await idbOpen();
  return new Promise((resolve, reject) => {
    const r = idbStore(db, 'readonly').getAll();
    r.onsuccess = () => resolve(r.result || []);
    r.onerror = () => reject(r.error);
  });
}

async function idbAdd(rec) {
  const db = await idbOpen();
  return new Promise((resolve, reject) => {
    const r = idbStore(db, 'readwrite').add(rec);
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

async function idbDelete(id) {
  const db = await idbOpen();
  return new Promise((resolve, reject) => {
    const r = idbStore(db, 'readwrite').delete(id);
    r.onsuccess = () => resolve();
    r.onerror = () => reject(r.error);
  });
}

async function idbPut(rec) {
  const db = await idbOpen();
  return new Promise((resolve, reject) => {
    const r = idbStore(db, 'readwrite').put(rec);
    r.onsuccess = () => resolve();
    r.onerror = () => reject(r.error);
  });
}

// Pull a human label out of a urlencoded message body for the queued /
// failed UI. Falls back to "[attachment]" when there's no text (file-only
// send).
function bodyLabel(body, contentType) {
  try {
    if (contentType && contentType.indexOf('application/x-www-form-urlencoded') === 0) {
      const text = new URLSearchParams(body).get('body') || '';
      const trimmed = text.trim();
      if (!trimmed) return '[attachment]';
      return trimmed.length > 60 ? trimmed.slice(0, 60) + '…' : trimmed;
    }
  } catch (e) {}
  return '[message]';
}

async function handleMessagePost(event) {
  const req = event.request;
  const replica = req.clone();
  try {
    return await fetch(req);
  } catch (e) {
    // Network unreachable: queue the request and report success so the
    // page's composer clears as if it had sent.
    const body = await replica.text();
    const contentType = replica.headers.get('Content-Type') || 'application/x-www-form-urlencoded';
    await idbAdd({
      url: req.url,
      body,
      contentType,
      label: bodyLabel(body, contentType),
      status: null, // null = pending; a number = failed-on-replay HTTP status
      ts: Date.now(),
    });
    await broadcastState();
    if (self.registration.sync) {
      try { await self.registration.sync.register('lc-outbox'); } catch (e2) {}
    }
    return new Response('{"queued":true}', {
      status: 200,
      headers: { 'Content-Type': 'application/json', 'X-LC-Queued': '1' },
    });
  }
}

async function drainOutbox() {
  const items = await idbAll();
  for (const it of items) {
    if (it.status !== null) continue; // leave failed items for explicit retry
    let res;
    try {
      res = await fetch(it.url, {
        method: 'POST',
        headers: { 'Content-Type': it.contentType },
        body: it.body,
        credentials: 'same-origin',
      });
    } catch (e) {
      break; // still offline: stop draining, keep the rest queued
    }
    if (res.ok) {
      await idbDelete(it.id);
    } else {
      // Server rejected it (room gone, muted, ...): mark failed so the page
      // can offer retry / discard instead of silently looping.
      it.status = res.status;
      await idbPut(it);
    }
  }
  await broadcastState();
}

async function broadcastState() {
  const items = await idbAll();
  const pending = items.filter((i) => i.status === null).map((i) => ({ id: i.id, label: i.label }));
  const failed = items
    .filter((i) => i.status !== null)
    .map((i) => ({ id: i.id, label: i.label, status: i.status }));
  const clients = await self.clients.matchAll({ includeUncontrolled: true });
  for (const c of clients) {
    c.postMessage({ type: 'lc-outbox-state', pending, failed });
  }
}

self.addEventListener('sync', (event) => {
  if (event.tag === 'lc-outbox') event.waitUntil(drainOutbox());
});

self.addEventListener('message', (event) => {
  const d = event.data || {};
  if (d.type === 'lc-outbox-drain') {
    event.waitUntil(drainOutbox());
  } else if (d.type === 'lc-outbox-get') {
    event.waitUntil(broadcastState());
  } else if (d.type === 'lc-outbox-retry') {
    event.waitUntil((async () => {
      const items = await idbAll();
      const it = items.find((i) => i.id === d.id);
      if (it) { it.status = null; await idbPut(it); }
      await drainOutbox();
    })());
  } else if (d.type === 'lc-outbox-discard') {
    event.waitUntil((async () => { await idbDelete(d.id); await broadcastState(); })());
  }
});

// ---- Push (unchanged behaviour) --------------------------------------------

self.addEventListener('push', (event) => {
  if (!event.data) return;
  let payload;
  try { payload = event.data.json(); } catch (e) { return; }
  const target = (payload.data && payload.data.target_path) || '/';
  event.waitUntil((async () => {
    const visible = await self.clients.matchAll({ type: 'window', visible: true });
    const onTarget = visible.some((c) => {
      try { return new URL(c.url).pathname === target; } catch (e) { return false; }
    });
    if (onTarget) return; // user is already reading the room
    return self.registration.showNotification(payload.title || 'lets-chat', {
      body: payload.body || '',
      icon: payload.icon,
      tag: payload.tag,
      data: payload.data || {},
    });
  })());
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const target = (event.notification.data && event.notification.data.target_path) || '/';
  event.waitUntil((async () => {
    const all = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    for (const c of all) {
      try {
        if (new URL(c.url).pathname === target) {
          await c.focus();
          return;
        }
      } catch (e) {}
    }
    await self.clients.openWindow(target);
  })());
});
