// LC-1084: run with `just test-js` (node --test). invite_modal.html's
// window.__lcOpenInviteModal lives in an inline <script>, not a standalone
// asset, so this extracts that block from the template source and evaluates
// it in a VM sandbox, the same approach add_room_modal.test.js uses for
// add_room_modal.html. Pins the fix for the same race the add-room modal was
// fixed for in LC-1077: a boosted #main navigation lands before the
// sidebar's out-of-band swap resolves, so the sidebar's Invite button still
// passes the PREVIOUS enclave id for a brief window. __lcOpenInviteModal must
// prefer window.LetsChatPageContext (live.js's synchronous read of #main)
// over that stale argument.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function extractScript() {
  const src = fs.readFileSync(
    path.join(__dirname, '..', 'templates', 'partials', 'invite_modal.html'),
    'utf8'
  );
  const m = /<script data-lc-guard="none">([\s\S]*?)<\/script>/.exec(src);
  assert.ok(m, 'invite_modal.html must contain a <script> block');
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
      value: '',
      innerHTML: '',
      __attrs: attrs,
      __listeners: listeners,
    },
    overrides
  );
}

function load(pageContext) {
  const script = extractScript();
  const search = fakeElement({});
  const results = fakeElement({});
  const closeBtn = fakeElement({});
  const modal = fakeElement({
    querySelector: (sel) => {
      if (sel === '[data-lc-invite-search]') return search;
      if (sel === '#lc-invite-modal-results') return results;
      if (sel === '[data-lc-invite-close]') return closeBtn;
      return null;
    },
  });
  const window = {
    LetsChatPageContext: pageContext,
  };
  const document = {
    activeElement: null,
    querySelector: (sel) => (sel === '[data-lc-invite-modal]' ? modal : null),
    contains: () => true,
  };
  vm.runInNewContext(script, { window, document });
  return { window, search, modal, results };
}

test('prefers the live page context over a stale sidebar-sourced id', () => {
  // The sidebar's OOB swap has not landed: it still passes E1 (the enclave
  // the page navigated AWAY from), but #main has already landed on E2.
  const t = load({ get: () => ({ type: 'page_context', room_id: null, enclave_id: 2 }) });
  t.window.__lcOpenInviteModal(1);
  assert.equal(t.search.getAttribute('hx-get'), '/enclave/2/invite/search');
});

test('falls back to the passed id when the page-context module is unavailable', () => {
  const t = load(null);
  t.window.__lcOpenInviteModal(5);
  assert.equal(t.search.getAttribute('hx-get'), '/enclave/5/invite/search');
});

test('leaves the hx-get alone when neither source names an enclave', () => {
  const t = load({ get: () => ({ type: 'page_context', room_id: null, enclave_id: null }) });
  t.window.__lcOpenInviteModal(null);
  assert.equal(t.search.getAttribute('hx-get'), null);
});
