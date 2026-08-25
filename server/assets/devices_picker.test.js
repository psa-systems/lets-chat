// LC-632: run with `just test-js` (node --test). devices.js is a browser IIFE
// that touches window/document at load, so it cannot be required in Node; this
// asserts on its source instead - a regression guard that the call-devices
// picker keeps the dark-themed, icon-labelled treatment and never falls back to
// the inverted light default that this ticket fixed.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const src = fs.readFileSync(path.join(__dirname, 'devices.js'), 'utf8');

test('call-devices picker uses dark theme tokens, not the inverted light defaults', () => {
  assert.ok(!/bg-white/.test(src), 'picker must not hardcode bg-white');
  assert.ok(!/(border|text|bg)-slate-\d/.test(src), 'picker must not use slate palette classes');
  assert.ok(/bg-surface-elevated/.test(src), 'dialog uses the elevated surface token');
  assert.ok(/bg-surface-sunken/.test(src), 'the selects use the sunken surface token');
  assert.ok(/text-content\b/.test(src), 'text uses the content token');
});

test('device types render as icons with a chevron affordance', () => {
  for (const kind of ['audioinput', 'videoinput', 'audiooutput']) {
    assert.match(src, new RegExp(kind + ":\\s*'<svg"), kind + ' has an icon glyph');
  }
  assert.ok(/var CHEVRON =/.test(src), 'a chevron affordance is defined');
  assert.ok(/appearance-none/.test(src), 'the select drops its native arrow for the chevron');
});

// LC-768: background blur. devices.js is a browser IIFE (window/document at
// load), so its behavior is guarded on source; the pure constraint logic it
// delegates to is unit-tested in media_constraints.test.js.

test('blur preference persists in localStorage like the device pins', () => {
  assert.match(src, /BLUR_KEY\s*=\s*'lc\.dev\.videoblur'/, 'a dedicated storage key');
  assert.match(src, /function getBlur\(\)[\s\S]*localStorage\.getItem\(BLUR_KEY\)/, 'reads the pref');
  assert.match(src, /function setBlur\([\s\S]*localStorage\.setItem\(BLUR_KEY/, 'writes the pref');
});

test('blur is feature-detected at acquisition, never by user agent', () => {
  assert.match(src, /getSupportedConstraints\(\)\.backgroundBlur/, 'detects the native constraint');
  assert.ok(!/userAgent|navigator\.platform/.test(src), 'no user-agent sniffing');
});

test('unsupported client hides the toggle (no inert control)', () => {
  // The blur row is appended only under a blurSupported() guard.
  assert.match(src, /blurSupported\(\)\)\s*{\s*[\s\S]*?blurRow\(\)/, 'toggle gated on support');
});

test('blur applies to both the call path and the camera-only restore path', () => {
  // Both acquisition helpers build video through the shared videoConstraint,
  // which carries the blur request; getCamera is the camera toggle / screen-
  // share restore path.
  const gum = src.match(/function getUserMedia[\s\S]*?\n  }/);
  const cam = src.match(/function getCamera[\s\S]*?\n  }/);
  assert.ok(gum && /videoConstraint\(/.test(gum[0]), 'call path uses videoConstraint');
  assert.ok(cam && /videoConstraint\(/.test(cam[0]), 'camera-restore path uses videoConstraint');
});

test('the frame-rate floor drops blur and informs the user', () => {
  assert.match(src, /BLUR_FPS_FLOOR/, 'a frame-rate floor is defined');
  assert.match(src, /requestVideoFrameCallback/, 'delivered frames are measured');
  assert.match(src, /applyConstraints\(\{\s*advanced:\s*\[\{\s*backgroundBlur:\s*false/, 'drops the effect live');
  assert.match(src, /__lcToast[\s\S]*deviceBlurSlow/, 'tells the user');
});
