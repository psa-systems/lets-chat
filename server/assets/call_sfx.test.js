// LC-1063: unit tests for the pure duration formatter shared by the 1-1 call
// summary (call.js) and exposed via call_sfx.js. Run with `just test-js`
// (node --test). The audio surface is a no-op under Node (no window), so only
// the pure helper is exercised here.
const test = require('node:test');
const assert = require('node:assert/strict');

const { formatDuration } = require('./call_sfx.js');

test('formatDuration: sub-minute shows m:ss with zero minutes', () => {
  assert.equal(formatDuration(0), '0:00');
  assert.equal(formatDuration(5), '0:05');
  assert.equal(formatDuration(59), '0:59');
});

test('formatDuration: minutes and seconds zero-pad the seconds only', () => {
  assert.equal(formatDuration(60), '1:00');
  assert.equal(formatDuration(75), '1:15');
  assert.equal(formatDuration(600), '10:00');
  assert.equal(formatDuration(629), '10:29');
});

test('formatDuration: an hour or more switches to h:mm:ss', () => {
  assert.equal(formatDuration(3600), '1:00:00');
  assert.equal(formatDuration(3661), '1:01:01');
  assert.equal(formatDuration(7384), '2:03:04');
});

test('formatDuration: floors fractional seconds and clamps negatives', () => {
  assert.equal(formatDuration(9.9), '0:09');
  assert.equal(formatDuration(-5), '0:00');
});

test('formatDuration: non-numeric input falls back to zero', () => {
  assert.equal(formatDuration(undefined), '0:00');
  assert.equal(formatDuration(null), '0:00');
  assert.equal(formatDuration('nope'), '0:00');
});
