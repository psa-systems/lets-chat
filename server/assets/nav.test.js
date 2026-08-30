// LC-837: run with `just test-js` (node --test). nav.js is a browser IIFE; it is
// evaluated in a VM sandbox whose document.body records the listeners it
// registers, so a fake htmx event can be pushed through them. Pins the two
// decisions a boosted navigation depends on: a response with no #main (login
// page, error page, 4xx/5xx) becomes a real navigation instead of a broken
// swap, and a response's branding <style> reaches the surviving <head>.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'nav.js'), 'utf8');
  const listeners = {};
  const assigned = [];
  const brand = { textContent: ':root{--brand-primary:#111111}' };
  const closed = { count: 0 };
  const window = {
    htmx: { config: { historyCacheSize: 10 } },
    location: { assign: (u) => assigned.push(u) },
    lcCloseNav: () => { closed.count += 1; },
  };
  const document = {
    body: {
      addEventListener: (type, fn) => {
        (listeners[type] = listeners[type] || []).push(fn);
      },
    },
    head: { querySelector: (sel) => (sel === 'style[data-lc-brand]' ? brand : null) },
  };
  vm.runInNewContext(src, { window, document });
  const fire = (type, detail, target) => {
    for (const fn of listeners[type] || []) fn({ type, detail, target });
  };
  return { window, listeners, assigned, brand, closed, fire };
}

const page = '<html><head><style data-lc-brand>:root{--brand-primary:#abcdef}</style></head><body><main id="main" hx-history-elt>hi</main></body></html>';
const login = '<html><head></head><body><form action="/login"></form></body></html>';

test('history cache is off so back/forward re-fetch the page', () => {
  const t = load();
  assert.equal(t.window.htmx.config.historyCacheSize, 0);
});

test('the pure decisions', () => {
  const t = load();
  const nav = t.window.LetsChatNav;
  assert.equal(nav.hasMain(page), true);
  assert.equal(nav.hasMain(login), false);
  assert.equal(nav.hasMain('<div id="main">'), false, 'only the <main> element counts');
  assert.equal(nav.brandCss(page), ':root{--brand-primary:#abcdef}');
  assert.equal(nav.brandCss(login), null);
  assert.equal(
    nav.fallbackUrl({ xhr: { responseURL: 'https://x/login' }, requestConfig: { path: '/room/1' } }),
    'https://x/login',
    'the URL the response came from, after redirects, wins'
  );
  assert.equal(nav.fallbackUrl({ xhr: {}, requestConfig: { path: '/room/1' } }), '/room/1');
});

test('a boosted response without #main becomes a real navigation', () => {
  const t = load();
  const d = { boosted: true, shouldSwap: true, xhr: { status: 200, responseText: login, responseURL: 'https://x/login' } };
  t.fire('htmx:beforeSwap', d);
  assert.equal(d.shouldSwap, false);
  assert.deepEqual(t.assigned, ['https://x/login']);
});

test('a boosted error response becomes a real navigation even if it carries a main', () => {
  const t = load();
  const d = { boosted: true, shouldSwap: false, xhr: { status: 404, responseText: page, responseURL: 'https://x/room/9' } };
  t.fire('htmx:beforeSwap', d);
  assert.deepEqual(t.assigned, ['https://x/room/9']);
});

test('a boosted page swaps and carries its branding into head', () => {
  const t = load();
  const d = { boosted: true, shouldSwap: true, xhr: { status: 200, responseText: page, responseURL: 'https://x/room/9' } };
  t.fire('htmx:beforeSwap', d);
  assert.equal(d.shouldSwap, true);
  assert.deepEqual(t.assigned, []);
  assert.equal(t.brand.textContent, ':root{--brand-primary:#abcdef}');
});

test('an unboosted swap is left alone', () => {
  const t = load();
  const d = { boosted: false, shouldSwap: true, xhr: { status: 200, responseText: '<div>fragment</div>' } };
  t.fire('htmx:beforeSwap', d);
  assert.equal(d.shouldSwap, true);
  assert.deepEqual(t.assigned, []);
  assert.equal(t.brand.textContent, ':root{--brand-primary:#111111}');
});

test('a #main swap and a history restore close the mobile nav', () => {
  const t = load();
  t.fire('htmx:afterSwap', { target: { id: 'main' } }, { id: 'main' });
  t.fire('htmx:afterSwap', { target: { id: 'sidebar-search-results' } }, { id: 'sidebar-search-results' });
  t.fire('htmx:historyRestore', {});
  assert.equal(t.closed.count, 2);
});

// LC-842: the admin support badge inherited the boosted tile's target and its
// load request deleted <main>. The template now declares its own target, and
// nav.js refuses any non-boosted swap that would replace #main with a
// response that has no <main id="main">, without navigating away.
test('a non-boosted request aimed at #main whose response has no main is refused, not navigated', () => {
  const t = load();
  const warned = [];
  t.window.console = { warn: (...a) => warned.push(a.join(' ')) };
  const d = {
    boosted: false,
    shouldSwap: true,
    target: { id: 'main' },
    requestConfig: { path: '/admin/support/badge' },
    xhr: { status: 200, responseText: '<span class="badge">3</span>', responseURL: 'https://x/admin/support/badge' },
  };
  t.fire('htmx:beforeSwap', d);
  assert.equal(d.shouldSwap, false);
  assert.deepEqual(t.assigned, [], 'a fragment request is never turned into a navigation');
  assert.equal(warned.length, 1);
  assert.match(warned[0], /admin\/support\/badge/);
});

test('a non-boosted request aimed at #main whose response carries a main still swaps', () => {
  const t = load();
  const d = { boosted: false, shouldSwap: true, target: { id: 'main' }, xhr: { status: 200, responseText: page } };
  t.fire('htmx:beforeSwap', d);
  assert.equal(d.shouldSwap, true, 'the LC-318 soft refresh of a full page keeps working');
});

test('a non-boosted request aimed elsewhere is untouched', () => {
  const t = load();
  const d = { boosted: false, shouldSwap: true, target: { id: 'lc-rail-support-badge' }, xhr: { status: 200, responseText: '<span>3</span>' } };
  t.fire('htmx:beforeSwap', d);
  assert.equal(d.shouldSwap, true);
  assert.deepEqual(t.assigned, []);
});
