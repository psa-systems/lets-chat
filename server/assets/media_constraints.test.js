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
