// LC-628: run with `node --test server/assets/media_constraints.test.js`
// (or `just test-js`). Asserts every mic path inherits echo cancellation,
// noise suppression, and auto gain control, and that pinning a device does not
// drop those flags.
const test = require('node:test');
const assert = require('node:assert/strict');
const media = require('./media-constraints.js');

test('audio constraint requests echo cancellation, noise suppression, auto gain', () => {
  const c = media.audio();
  assert.equal(c.echoCancellation, true);
  assert.equal(c.noiseSuppression, true);
  assert.equal(c.autoGainControl, true);
  assert.ok(!('deviceId' in c), 'no pin should not add a deviceId constraint');
});

test('a pinned deviceId is merged as an exact constraint without dropping the flags', () => {
  const c = media.audio('mic-42');
  assert.deepEqual(c.deviceId, { exact: 'mic-42' });
  assert.equal(c.echoCancellation, true);
  assert.equal(c.noiseSuppression, true);
  assert.equal(c.autoGainControl, true);
});

test('each call returns a fresh object (no shared mutable constraint)', () => {
  const a = media.audio();
  const b = media.audio();
  assert.notEqual(a, b);
});

// LC-768: background blur video constraint.

test('video with no pin and no blur is the bare `true` the callers relied on', () => {
  assert.equal(media.video('', { blur: false, blurSupported: true }), true);
  assert.equal(media.video(''), true);
});

test('a pinned camera is an exact deviceId constraint', () => {
  assert.deepEqual(media.video('cam-7'), { deviceId: { exact: 'cam-7' } });
});

test('blur is requested only when wanted AND the browser supports it', () => {
  // Wanted + supported: the constraint carries backgroundBlur.
  assert.equal(media.video('', { blur: true, blurSupported: true }).backgroundBlur, true);
  assert.deepEqual(media.video('cam-7', { blur: true, blurSupported: true }), {
    deviceId: { exact: 'cam-7' },
    backgroundBlur: true,
  });
});

test('unsupported client never gets a blur constraint (published unblurred, not broken)', () => {
  // Wanted but unsupported: no backgroundBlur, and with no pin it degrades to
  // the bare `true` so a working stream is still acquired.
  assert.equal(media.video('', { blur: true, blurSupported: false }), true);
  assert.deepEqual(media.video('cam-7', { blur: true, blurSupported: false }), {
    deviceId: { exact: 'cam-7' },
  });
});

test('blur is advisory, not an exact constraint (so it can never overconstrain)', () => {
  const c = media.video('', { blur: true, blurSupported: true });
  assert.equal(c.backgroundBlur, true);
  assert.equal(typeof c.backgroundBlur, 'boolean', 'not wrapped in { exact: ... }');
});
