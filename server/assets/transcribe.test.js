// LC-843: run with `just test-js` (node --test). transcribe.js is a browser
// IIFE; it is evaluated in a VM sandbox with just enough DOM/fetch stubbed for
// its load-time wiring, then the exported pure engine decision is pinned. The
// matrix is the contract: the Accurate preference only wins when the server
// engine is really available, the Fast preference only wins when SR exists, a
// dead browser engine (serverFallback) beats any preference, and clients that
// never touch the control keep the pre-LC-843 automatic behavior.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'transcribe.js'), 'utf8');
  const stored = {};
  const els = { body: { addEventListener() {}, classList: { toggle() {}, add() {}, remove() {} } } };
  const window = {};
  const document = {
    body: els.body,
    addEventListener() {},
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementById: () => null,
  };
  const sandbox = {
    window,
    document,
    localStorage: {
      getItem: (k) => (k in stored ? stored[k] : null),
      setItem: (k, v) => { stored[k] = String(v); },
    },
    fetch: () => Promise.resolve({ ok: false }),
    navigator: {},
    console,
    setTimeout,
    clearTimeout,
  };
  vm.runInNewContext(src, sandbox);
  return { window, stored };
}

test('the engine decision matrix', () => {
  const choose = load().window.LetsChatTranscribe.chooseEngine;
  // (pref, hasSR, hasMR, sttServer, serverFallback)
  // Automatic (no preference): the pre-LC-843 behavior, unchanged.
  assert.equal(choose(null, true, true, true, false), 'browser', 'auto prefers the fast path');
  assert.equal(choose(null, true, true, false, false), 'browser');
  assert.equal(choose(null, false, true, true, false), 'server', 'no SR + operator STT = server');
  assert.equal(choose(null, false, true, false, false), 'browser', 'nothing available degrades like before');
  // Accurate preference: honored only when the server engine really exists.
  assert.equal(choose('accurate', true, true, true, false), 'server', 'the LC-843 switch');
  assert.equal(choose('accurate', true, true, false, false), 'browser', 'no operator STT: pref cannot conjure it');
  assert.equal(choose('accurate', true, false, true, false), 'browser', 'no MediaRecorder: pref cannot conjure it');
  // Fast preference: honored only when SR exists.
  assert.equal(choose('fast', true, true, true, false), 'browser');
  assert.equal(choose('fast', false, true, true, false), 'server', 'no SR: pref cannot conjure it');
  // A dead browser engine beats any preference.
  assert.equal(choose('fast', true, true, true, true), 'server', 'fallback outranks the fast pref');
  assert.equal(choose(null, true, true, true, true), 'server');
  assert.equal(choose(null, true, true, false, true), 'browser', 'fallback needs the server engine too');
});

test('a stored preference is read at load and only valid values count', () => {
  // The module reads localStorage at evaluation time; a garbage value must not
  // become a pref (chooseEngine only understands fast/accurate/null).
  const t = load();
  assert.equal(t.window.LetsChatTranscribe.chooseEngine('garbage', false, true, true, false), 'server',
    'an unknown pref falls through to the automatic rules');
  assert.equal(t.window.LetsChatTranscribe.chooseEngine('garbage', true, true, true, false), 'browser');
});
