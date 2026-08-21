// LC-764: run with `node --test server/assets/mute-state.test.js` (or
// `just test-js`). Asserts the mute button's pressed state is always taken from
// the track's real post-toggle reading, and that a rejected or no-op toggle is
// reported as failed so the call UI can surface it.
const test = require('node:test');
const assert = require('node:assert/strict');
const mute = require('./mute-state.js');

test('a successful unmute (enabled false -> true) reads unmuted and does not fail', () => {
  const r = mute.nextState(false, true, true);
  assert.equal(r.muted, false);
  assert.equal(r.failed, false);
});

test('a successful mute (enabled true -> false) reads muted and does not fail', () => {
  const r = mute.nextState(true, false, true);
  assert.equal(r.muted, true);
  assert.equal(r.failed, false);
});

test('a rejected unmute keeps the real (still-disabled) state and reports failure', () => {
  // setMicrophoneEnabled(true) rejected: the track never enabled, so the button
  // must read muted AND the failure must be surfaced - not silently swallowed.
  const r = mute.nextState(false, false, false);
  assert.equal(r.muted, true, 'still muted because the track never enabled');
  assert.equal(r.failed, true, 'a rejected toggle must be reported as failed');
});

test('a resolved-but-no-op toggle (state did not change) is reported as failed', () => {
  // The promise resolved yet the enabled reading did not flip: still a failure,
  // so the button never claims a state the track does not hold.
  const r = mute.nextState(false, false, true);
  assert.equal(r.muted, true);
  assert.equal(r.failed, true);
});
