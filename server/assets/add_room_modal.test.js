// LC-1077: run with `just test-js` (node --test). add_room_modal.html's
// window.__lcOpenAddRoom lives in an inline <script>, not a standalone asset,
// so this extracts that block from the template source and evaluates it in a
// VM sandbox, the same approach nav.test.js uses for nav.js. Pins the fix for
// the narrowed add-room race: a boosted #main navigation lands before the
// sidebar's out-of-band swap resolves, so the sidebar's `+` button still
// passes the PREVIOUS enclave id for a brief window. __lcOpenAddRoom must
// prefer window.LetsChatPageContext (live.js's synchronous read of #main,
// LC-1077) over that stale argument.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function extractScript() {
  const src = fs.readFileSync(
    path.join(__dirname, '..', 'templates', 'add_room_modal.html'),
    'utf8'
  );
  const m = /<script>([\s\S]*?)<\/script>/.exec(src);
  assert.ok(m, 'add_room_modal.html must contain a <script> block');
  return m[1];
}

function fakeElement(overrides) {
  const classes = new Set();
  const attrs = {};
  const listeners = {};
  return Object.assign(
    {
      classList: {
        contains: (c) => classes.has(c),
        add: (c) => classes.add(c),
        remove: (c) => classes.delete(c),
      },
      setAttribute: (k, v) => { attrs[k] = v; },
      getAttribute: (k) => (k in attrs ? attrs[k] : null),
      addEventListener: (type, fn) => { (listeners[type] = listeners[type] || []).push(fn); },
      querySelector: () => null,
      focus: () => {},
      reset: () => {},
      __attrs: attrs,
      __listeners: listeners,
    },
    overrides
  );
}

function load(pageContext) {
  const script = extractScript();
  const nameInput = fakeElement({});
  const closeBtn = fakeElement({});
  const cancelBtn = fakeElement({});
  const form = fakeElement({});
  const modal = fakeElement({
    querySelector: (sel) => {
      if (sel === 'form') return form;
      if (sel === '[data-lc-addroom-name]') return nameInput;
      if (sel === '[data-lc-addroom-close]') return closeBtn;
      if (sel === '[data-lc-addroom-cancel]') return cancelBtn;
      return null;
    },
  });
  const window = {
    LetsChatPageContext: pageContext,
  };
  const document = {
    activeElement: null,
    querySelector: (sel) => (sel === '[data-lc-addroom-modal]' ? modal : null),
    contains: () => true,
  };
  vm.runInNewContext(script, { window, document });
  return { window, form, modal, nameInput };
}

test('prefers the live page context over a stale sidebar-sourced id', () => {
  // The sidebar's OOB swap has not landed: it still passes E1 (the enclave
  // the page navigated AWAY from), but #main has already landed on E2.
  const t = load({ get: () => ({ type: 'page_context', room_id: null, enclave_id: 2 }) });
  t.window.__lcOpenAddRoom(1);
  assert.equal(t.form.getAttribute('action'), '/enclave/2/rooms');
});

test('falls back to the passed id when the page-context module is unavailable', () => {
  const t = load(null);
  t.window.__lcOpenAddRoom(5);
  assert.equal(t.form.getAttribute('action'), '/enclave/5/rooms');
});

test('leaves the action alone when neither source names an enclave', () => {
  const t = load({ get: () => ({ type: 'page_context', room_id: null, enclave_id: null }) });
  t.window.__lcOpenAddRoom(null);
  assert.equal(t.form.getAttribute('action'), null);
});
