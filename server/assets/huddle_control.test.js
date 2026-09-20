// LC-853: run with `just test-js` (node --test). huddle_control.js is a
// browser IIFE; it is evaluated in a VM sandbox with just enough DOM stubbed
// for its load-time wiring (no huddle bar present, so it exports and stops),
// then the pure request-eligibility decision is pinned. The matrix is the
// contract: the affordance exists only while joined with exactly ONE other
// participant sharing - zero sharers, several sharers, or "the sharer is me"
// all refuse, mirroring the server's fail-closed routing.
//
// LC-931: two further tests build control markup in a DETACHED tree (never
// reachable from the sandbox's own `document`, standing in for a huddle
// dock popped into a PiP window's separate document) and point the module at
// it via bindRoot(). They only pass if every lookup resolves through the
// captured root rather than a `document.querySelector` call: run against the
// pre-LC-931 module (root() re-queried `document` on every call) they fail,
// either on the missing `bindRoot` export or on the assertion that follows.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'huddle_control.js'), 'utf8');
  let controls = {};
  const buses = {};
  const wsMessages = [];
  const window = {
    LetsChatRtc: {
      watchBus: (busId, attr, handler) => { buses[busId] = handler; },
      bindControls: (map) => { controls = map; },
      setLabel: () => {},
      control: {
        events: { START: 'lc:control-start', INPUT: 'lc:control-input', END: 'lc:control-end', KILL: 'lc:control-kill' },
      },
    },
    LetsChatVoice: { isJoined: () => true },
    __lcS: null,
    __lcWS: { send: (msg) => wsMessages.push(JSON.parse(msg)) },
  };
  const document = {
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementById: () => null,
    addEventListener() {},
    dispatchEvent: () => true,
    createElement: (t) => new El(t),
  };
  const sandbox = {
    window,
    document,
    console,
    setTimeout,
    clearTimeout,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init && init.detail; } },
  };
  vm.runInNewContext(src, sandbox);
  return { window, controls, buses, wsMessages };
}

// A dock ([data-lc-voice-root]) containing the control span, a request
// button and one voice tile - the slice of room/huddle.html's markup this
// module actually touches. Never appended under the sandbox's `document`.
function buildDock(roomId, tileUid) {
  const dock = new El('div');
  dock.setAttribute('data-lc-voice-root', '');
  dock.setAttribute('data-self-id', 'me');
  dock.setAttribute('data-lc-huddle-sfu', '1');

  const control = new El('span');
  control.setAttribute('data-lc-huddle-control', '');
  control.setAttribute('data-lc-room', String(roomId));
  const requestBtn = new El('button');
  requestBtn.setAttribute('data-lc-huddle-control-request', '');
  control.appendChild(requestBtn);
  dock.appendChild(control);

  const tile = new El('div');
  tile.setAttribute('data-lc-voice-tile', tileUid);
  dock.appendChild(tile);

  return { dock, tile };
}

test('request eligibility: exactly one sharer, who is not me, while joined', () => {
  const canRequest = load().window.LetsChatHuddleControl.canRequest;
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

test('resolves the room id from a dock bound in a detached document, not `document`', () => {
  const { window, controls, wsMessages } = load();
  const { dock } = buildDock(42, 'alice');

  window.LetsChatHuddleControl.bindRoot(dock);
  controls['[data-lc-huddle-control-request]']();

  assert.deepEqual(wsMessages, [{ type: 'remote_control_signal', room_id: 42, kind: 'request' }]);
});

test('the room-wide "is controlling" label renders on the tile of a dock bound from a detached document', () => {
  const { window, buses } = load();
  const { dock, tile } = buildDock(42, 'alice');

  window.LetsChatHuddleControl.bindRoot(dock);

  const evt = new El('div');
  evt.setAttribute('data-room-id', '42');
  evt.setAttribute('data-kind', 'control');
  evt.setAttribute('data-user-id', 'alice');
  evt.setAttribute('data-username', 'Alice');
  evt.setAttribute('data-payload', '1');
  buses['lc-voice-bus'](evt);

  const label = tile.querySelector('[data-lc-control-label]');
  assert.ok(label, 'expected a control label on the tile');
  assert.equal(label.textContent, 'Alice is controlling');
});
