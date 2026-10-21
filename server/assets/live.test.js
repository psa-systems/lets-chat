// LC-1077: run with `just test-js` (node --test). live.js is a browser IIFE
// loaded once from base.html; it is evaluated in a VM sandbox whose
// document.body records the listeners it registers, the same pattern
// nav.test.js uses. Pins that window.LetsChatPageContext.get() mirrors the
// same page_context shape live.js sends over the socket, so the add-room
// modal (add_room_modal.test.js) can read the viewer's CURRENT enclave
// without waiting on the sidebar's out-of-band swap.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load(rootQuerySelector) {
  const src = fs.readFileSync(path.join(__dirname, 'live.js'), 'utf8');
  const listeners = {};
  const window = {};
  const document = {
    addEventListener: () => {},
    body: {
      addEventListener: (type, fn) => { (listeners[type] = listeners[type] || []).push(fn); },
    },
    documentElement: { classList: { add: () => {}, remove: () => {}, contains: () => false } },
    querySelector: rootQuerySelector,
    querySelectorAll: () => [],
  };
  vm.runInNewContext(src, { window, document });
  return { window, document, listeners };
}

test('LetsChatPageContext.get() reads the enclave topic off the current page', () => {
  const t = load((sel) => {
    if (sel === '[data-lc-live-topic^="enclave:"]') {
      return { getAttribute: () => 'enclave:42' };
    }
    return null;
  });
  const ctx = t.window.LetsChatPageContext.get();
  assert.equal(ctx.type, 'page_context');
  assert.equal(ctx.room_id, null);
  assert.equal(ctx.enclave_id, 42);
});

test('LetsChatPageContext.get() falls back to data-lc-page-enclave (LC-1051)', () => {
  const t = load((sel) => {
    if (sel === '[data-lc-page-enclave]') {
      return { getAttribute: () => '7' };
    }
    return null;
  });
  assert.equal(t.window.LetsChatPageContext.get().enclave_id, 7);
});

test('LetsChatPageContext.get() reports no enclave when the page names none', () => {
  const t = load(() => null);
  const ctx = t.window.LetsChatPageContext.get();
  assert.equal(ctx.type, 'page_context');
  assert.equal(ctx.room_id, null);
  assert.equal(ctx.enclave_id, null);
});
