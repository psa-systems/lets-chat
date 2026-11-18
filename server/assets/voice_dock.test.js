// LC-1195: unit tests for the unified call dock controller (voice_dock.js).
//
// The dock's container behaviour is DOM-heavy and coordinates with transcribe.js,
// so the full flow is exercised by hand in the browser. What is pinned here is the
// pure tab resolver - including the feature-flag rule that a Diagnostics tab which
// is not rendered (flag off) can never be the active tab - and that the module is
// an inert no-op on a page with no dock (every other page).
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

// Minimal sandbox: just enough for the IIFE to run to completion and export
// window.LetsChatDock. No [data-lc-call-dock] in the DOM, so the module stays a
// no-op (as on any non-voice page).
function load() {
  const body = new El('body');
  const store = new Map();
  const document = {
    body,
    addEventListener: () => {},
    getElementById: () => null,
    querySelector: () => null,      // no dock on the page
    querySelectorAll: () => [],
  };
  const window = {};
  const sandbox = {
    window,
    document,
    localStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
    },
    MutationObserver: class { observe() {} disconnect() {} },
    setTimeout,
    clearTimeout,
    console: { warn() {}, log() {}, error() {} },
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(path.join(__dirname, 'voice_dock.js'), 'utf8'), sandbox);
  return { window, document, body };
}

test('LC-1195: resolveTab picks the active tab, and a flag-off (absent) diag tab can never win', () => {
  const { window } = load();
  const r = window.LetsChatDock.resolveTab;
  // Transcript showing always wins, whatever the remembered body tab.
  assert.equal(r(true, 'chat', true), 'transcript');
  assert.equal(r(true, 'diag', true), 'transcript');
  // Transcript closed: the remembered body tab shows...
  assert.equal(r(false, 'chat', true), 'chat');
  assert.equal(r(false, 'diag', true), 'diag');
  // ...but 'diag' only when the Diagnostics tab is actually present. With the
  // CALL_DIAGNOSTICS flag off the tab is not rendered, so it falls back to chat
  // - the dock can never land on a tab that does not exist.
  assert.equal(r(false, 'diag', false), 'chat');
  assert.equal(r(false, 'chat', false), 'chat');
});

test('LC-1195: with no dock on the page the module is an inert no-op', () => {
  const { window, body } = load();
  // It still exports the API (loaded globally), but touching it does nothing and
  // never throws on a page that has no dock.
  assert.equal(typeof window.LetsChatDock.openTo, 'function');
  assert.equal(window.LetsChatDock.isChatVisible(), false);
  assert.doesNotThrow(() => window.LetsChatDock.openTo('chat'));
  assert.doesNotThrow(() => window.LetsChatDock.collapse());
  assert.doesNotThrow(() => window.LetsChatDock.setUnread(true));
  // The page marker is only added when a dock exists.
  assert.equal(body.classList.contains('lc-has-call-dock'), false);
});
