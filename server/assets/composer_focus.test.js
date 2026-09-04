// LC-858: run with `just test-js` (node --test). Loads the real
// composer_focus.js over a hand-built fake document (test_dom.js has no focus /
// activeElement support), the same idiom as nav.test.js, and drives the
// htmx:afterSettle events a room navigation fires.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// A fake textarea whose focus() records the call and becomes the document's
// activeElement, so "already focused" and "focus moved away" are observable.
function makeComposer(doc, opts) {
  opts = opts || {};
  return {
    tagName: 'TEXTAREA',
    disabled: !!opts.disabled,
    focuses: 0,
    lastArg: undefined,
    focus(arg) {
      this.focuses += 1;
      this.lastArg = arg;
      doc.activeElement = this;
    },
  };
}

function load(opts) {
  opts = opts || {};
  const listeners = {};
  const addTo = (bag) => (type, fn) => {
    (bag[type] = bag[type] || []).push(fn);
  };

  const document = {
    readyState: opts.readyState || 'complete',
    activeElement: null,
    // The composer for the current room, or null in a read-only room.
    _composer: null,
    addEventListener: addTo(listeners),
    // Only the composer selector resolves; everything else is absent.
    querySelector(sel) {
      return sel.indexOf('textarea[name="body"]') !== -1 ? this._composer : null;
    },
    body: { addEventListener: addTo(listeners) },
  };
  if (!opts.noComposer) {
    document._composer = makeComposer(document, { disabled: opts.disabled });
  }

  const window = {};
  const src = fs.readFileSync(path.join(__dirname, 'composer_focus.js'), 'utf8');
  vm.runInNewContext(src, { window, document });

  const fire = (type, target) => {
    for (const fn of listeners[type] || []) fn({ type, target });
  };
  return {
    document,
    fire,
    composer: () => document._composer,
    // Simulate a room swap: #main (with the old composer) is replaced, so a
    // fresh composer element exists and focus has reset to the document body.
    swapRoom() {
      document._composer = makeComposer(document);
      document.activeElement = null;
      fire('htmx:afterSettle', { id: 'main' });
    },
  };
}

test('LC-858: entering a room (initial load) focuses the composer', () => {
  const t = load({ readyState: 'complete' });
  assert.equal(t.composer().focuses, 1, 'the composer is focused on entry');
  assert.equal(t.document.activeElement, t.composer());
  assert.equal(
    t.composer().lastArg && t.composer().lastArg.preventScroll,
    true,
    'focus does not scroll the page',
  );
});

test('LC-858: switching rooms refocuses the freshly rendered composer', () => {
  const t = load();
  t.swapRoom();
  assert.equal(t.composer().focuses, 1, 'the new room’s composer is focused after the swap');
  assert.equal(t.document.activeElement, t.composer());
});

test('LC-858: focus holds across repeated room switches, not just the first', () => {
  const t = load();
  t.swapRoom();
  const first = t.composer();
  t.swapRoom();
  const second = t.composer();
  assert.notEqual(first, second, 'each switch renders a new composer element');
  assert.equal(second.focuses, 1, 'the second switch also lands the caret');
  assert.equal(t.document.activeElement, second);
});

test('LC-858: an out-of-band swap (not #main) never steals focus', () => {
  const t = load();
  // The user is typing somewhere else; a WS-driven OOB row/badge settles.
  const elsewhere = { tagName: 'INPUT' };
  t.document.activeElement = elsewhere;
  const before = t.composer().focuses;
  t.fire('htmx:afterSettle', { id: 'sidebar' });
  t.fire('htmx:afterSettle', { id: 'lc-invitations' });
  assert.equal(t.composer().focuses, before, 'the composer was not touched by the OOB swaps');
  assert.equal(t.document.activeElement, elsewhere, 'focus stayed where the user put it');
});

test('LC-858: a later-mounting component that steals focus is beaten by the settle', () => {
  const t = load();
  t.swapRoom();
  assert.equal(t.document.activeElement, t.composer());
  // Something mounts after and grabs focus, then a subsequent settle for the
  // same room lands: the composer wins again.
  const thief = { tagName: 'BUTTON' };
  t.document.activeElement = thief;
  t.fire('htmx:afterSettle', { id: 'main' });
  assert.equal(t.document.activeElement, t.composer(), 'focus is restored to the composer');
});

test('LC-858: a read-only room (no composer) is a no-op, not a crash', () => {
  const t = load({ noComposer: true });
  assert.doesNotThrow(() => t.fire('htmx:afterSettle', { id: 'main' }));
});

test('LC-858: a disabled composer is left alone', () => {
  const t = load({ disabled: true });
  t.fire('htmx:afterSettle', { id: 'main' });
  assert.equal(t.composer().focuses, 0);
});

test('LC-858: already in the composer - no redundant refocus', () => {
  const t = load();
  const ta = t.composer();
  assert.equal(ta.focuses, 1); // initial
  t.document.activeElement = ta;
  t.fire('htmx:afterSettle', { id: 'main' });
  assert.equal(ta.focuses, 1, 'no second focus call while already there');
});
