// LC-853: run with `just test-js` (node --test). huddle_control.js is a
// browser IIFE; it is evaluated in a VM sandbox with just enough DOM stubbed
// for its load-time wiring (no huddle bar present, so it exports and stops),
// then the pure request-eligibility decision is pinned. The matrix is the
// contract: the affordance exists only while joined with exactly ONE other
// participant sharing - zero sharers, several sharers, or "the sharer is me"
// all refuse, mirroring the server's fail-closed routing.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'huddle_control.js'), 'utf8');
  const window = {};
  const document = {
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementById: () => null,
    addEventListener() {},
  };
  const sandbox = { window, document, console, setTimeout, clearTimeout };
  vm.runInNewContext(src, sandbox);
  return window;
}

test('request eligibility: exactly one sharer, who is not me, while joined', () => {
  const canRequest = load().LetsChatHuddleControl.canRequest;
  const base = { enabled: true, joined: true, selfId: 'me' };

  // The one shape that shows the affordance.
  assert.equal(canRequest({ ...base, sharers: { alice: true } }), true);

  // Nobody sharing: nothing to control.
  assert.equal(canRequest({ ...base, sharers: {} }), false);
  // A share that started and stopped is not a sharer.
  assert.equal(canRequest({ ...base, sharers: { alice: false } }), false);
  // Two sharers: ambiguous target, refused on both ends (server too).
  assert.equal(canRequest({ ...base, sharers: { alice: true, bob: true } }), false);
  // The sharer is me: I cannot request control of my own screen.
  assert.equal(canRequest({ ...base, sharers: { me: true } }), false);
  // One live sharer beside a stopped one still counts as exactly one.
  assert.equal(canRequest({ ...base, sharers: { alice: true, bob: false } }), true);

  // Not joined to the huddle: a spectator gets no affordance.
  assert.equal(canRequest({ ...base, joined: false, sharers: { alice: true } }), false);
  // Feature off (workspace switch): nothing, ever.
  assert.equal(canRequest({ ...base, enabled: false, sharers: { alice: true } }), false);
  // No self identity resolved: fail closed.
  assert.equal(canRequest({ enabled: true, joined: true, selfId: null, sharers: { alice: true } }), false);
  // Absent sharers map reads as nobody sharing, not a crash.
  assert.equal(canRequest({ ...base }), false);
});
