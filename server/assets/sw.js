// lets-chat service worker.
//
// Two responsibilities, nothing else:
//   1. push:              parse JSON payload, render OS notification.
//   2. notificationclick: focus or open the target tab.
//
// IMPORTANT: the push handler suppresses showNotification() when any
// visible client is already on data.target_path. This is *baked in*, not
// optional - the user is actively reading the room, an OS-level ping
// would be noise. If a future tweak wants to relax this, do so behind an
// explicit user setting.

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

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
