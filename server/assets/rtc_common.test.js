// LC-854: run with `just test-js` (node --test). rtc_common.js is a browser
// IIFE; evaluated in a VM sandbox with the DOM bits its load-time wiring
// touches stubbed, then the pure pieces of the shared remote-control capture
// are pinned: the key-forwarding policy (a security boundary - the kill hotkey
// and OS combos must never be forwarded) and the object-contain coordinate map.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'rtc_common.js'), 'utf8');
  const window = {};
  const document = { getElementById: () => null, body: { addEventListener() {} } };
  const sandbox = { window, document, console };
  vm.runInNewContext(src, sandbox);
  return window.LetsChatRtc.control;
}

function key(code, mods) {
  mods = mods || {};
  return {
    code,
    ctrlKey: !!mods.ctrl,
    shiftKey: !!mods.shift,
    altKey: !!mods.alt,
    metaKey: !!mods.meta,
  };
}

test('key-forwarding blocklist (LC-854 security boundary)', () => {
  const c = load();

  // The kill hotkey (Ctrl/Cmd+Alt+F9) is NEVER forwarded - forwarding it would
  // let the controller drive the sharer's own panic combo.
  assert.equal(c.isForwardableKey(key('F9', { ctrl: true, alt: true })), false);
  assert.equal(c.isForwardableKey(key('F9', { meta: true, alt: true })), false);
  // Plain F9, or F9 with only one of the modifiers, is an ordinary key.
  assert.equal(c.isForwardableKey(key('F9')), true);
  assert.equal(c.isForwardableKey(key('F9', { alt: true })), true);

  // OS combos the injector cannot synthesize are dropped, not sent as a
  // misleading no-op mid-stream.
  assert.equal(c.isForwardableKey(key('Delete', { ctrl: true, alt: true })), false);
  assert.equal(c.isForwardableKey(key('KeyL', { meta: true })), false);

  // Ordinary keys and app shortcuts ARE forwarded (they drive the peer's app;
  // the local preventDefault stops them firing on the controller).
  assert.equal(c.isForwardableKey(key('KeyC', { ctrl: true })), true);
  assert.equal(c.isForwardableKey(key('KeyW', { ctrl: true })), true);
  assert.equal(c.isForwardableKey(key('KeyA')), true);
  assert.equal(c.isForwardableKey(key('Delete')), true); // plain Delete is fine
});

test('modifier bitmask (ctrl=1 shift=2 alt=4 meta=8)', () => {
  const c = load();
  assert.equal(c.modMask(key('KeyA')), 0);
  assert.equal(c.modMask(key('KeyA', { ctrl: true })), 1);
  assert.equal(c.modMask(key('KeyA', { shift: true })), 2);
  assert.equal(c.modMask(key('KeyA', { alt: true })), 4);
  assert.equal(c.modMask(key('KeyA', { meta: true })), 8);
  assert.equal(c.modMask(key('KeyA', { ctrl: true, alt: true, meta: true })), 13);
});

test('normCoords maps object-contain letterboxing to [0,1], null in the bars', () => {
  const c = load();
  // A 200x100 source shown in a 200x200 box (contain): 50px letterbox top and
  // bottom, full width. Center of the box is the center of the surface.
  const video = {
    videoWidth: 200,
    videoHeight: 100,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 200, height: 200 }),
  };
  const mid = c.normCoords(video, 100, 100);
  assert.ok(Math.abs(mid.x - 0.5) < 1e-9 && Math.abs(mid.y - 0.5) < 1e-9);
  // Top-left of the actual content (y starts 50px down).
  const tl = c.normCoords(video, 0, 50);
  assert.ok(Math.abs(tl.x) < 1e-9 && Math.abs(tl.y) < 1e-9);
  // A point in the top letterbox bar (y=10) is outside the surface -> null.
  assert.equal(c.normCoords(video, 100, 10), null);
  // No dimensions yet (video not ready) -> null, never a bogus coord.
  assert.equal(c.normCoords({ videoWidth: 0, videoHeight: 0 }, 5, 5), null);
});
