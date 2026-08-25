// LC-774: run with `just test-js` (node --test). The service worker is not a
// module and cannot be required; it registers its handlers on `self` at load.
// So load the source, substitute `__ASSET_VERSION__` (as routes/push.rs does at
// serve time), and evaluate it in a VM sandbox with stub `self` / `caches` /
// `console`, capturing the listeners it registers. Then drive the `install`
// listener with a cache whose `add()` rejects for one URL, asserting the
// precache-failure logging LC-758 added: one warning naming the URL and the
// error, install still resolves, and every other entry is cached.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// Evaluate sw.js in a sandbox. `cacheAdd` is the stub `cache.add`; returns the
// captured listeners, the console.warn calls, and the URLs `add` was given.
function loadServiceWorker(cacheAdd) {
  const src = fs
    .readFileSync(path.join(__dirname, 'sw.js'), 'utf8')
    .replace(/__ASSET_VERSION__/g, 'testver');

  const listeners = {};
  const warnings = [];
  const cache = {
    add: cacheAdd,
    put: async () => {},
    match: async () => undefined,
  };
  const sandbox = {
    self: {
      addEventListener: (type, fn) => {
        (listeners[type] = listeners[type] || []).push(fn);
      },
      skipWaiting: async () => {},
      clients: { claim: async () => {}, matchAll: async () => [] },
      location: { origin: 'https://example.test' },
      registration: {},
    },
    caches: {
      open: async () => cache,
      keys: async () => [],
      delete: async () => true,
    },
    console: {
      warn: (...args) => warnings.push(args),
      log: () => {},
      error: () => {},
    },
    // Defensive stubs: not reached by the install path, but present so a stray
    // top-level reference would fail loudly rather than silently.
    indexedDB: { open: () => ({}) },
    URL,
    Response: class {},
    Request: class {},
    setTimeout,
    clearTimeout,
  };
  vm.createContext(sandbox);
  vm.runInContext(src, sandbox);
  return { listeners, warnings };
}

// The `install` handler stores its work-promise via event.waitUntil; return it
// so the test can await the real install completion.
function runInstall(listeners) {
  const handler = listeners.install && listeners.install[0];
  assert.ok(handler, 'sw.js registered no install listener');
  let work;
  handler({ waitUntil: (p) => { work = p; } });
  assert.ok(work, 'install handler did not call event.waitUntil');
  return work;
}

test('a failing precache add logs exactly one warning naming the URL and error, and install still resolves', async () => {
  const added = [];
  const failure = new Error('simulated 404');
  const { listeners, warnings } = loadServiceWorker(async (url) => {
    if (url.startsWith('/assets/offline.html')) throw failure;
    added.push(url);
  });

  // Install must resolve, not reject, even though one entry failed.
  await runInstall(listeners);

  // Exactly one warning, carrying the failing URL and the rejection reason.
  assert.equal(warnings.length, 1, 'expected one console.warn for the failed entry');
  const warned = warnings[0];
  assert.ok(
    warned.some((a) => typeof a === 'string' && a.includes('/assets/offline.html')),
    'the warning does not name the failing URL'
  );
  assert.ok(warned.includes(failure), 'the warning does not carry the rejection');

  // Every other PRECACHE_URLS entry was still cached.
  assert.ok(added.length >= 10, `expected the other entries to cache, got ${added.length}`);
  assert.ok(
    !added.some((u) => u.startsWith('/assets/offline.html')),
    'the failed entry should not appear as cached'
  );
});

test('with every add succeeding, install caches all entries and logs nothing', async () => {
  const added = [];
  const { listeners, warnings } = loadServiceWorker(async (url) => { added.push(url); });

  await runInstall(listeners);

  assert.equal(warnings.length, 0, 'a fully successful install must not warn');
  assert.ok(added.length >= 11, `expected every entry cached, got ${added.length}`);
});
