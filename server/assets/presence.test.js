// LC-1057: run with `just test-js` (node --test). presence.js is a browser IIFE;
// it runs in a VM sandbox whose document/window record their listeners, so fake
// input, socket-open and ping events can be pushed through them. Pins the
// activity throttle, the visibility gate, the reconnect rule and the pong reply.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load({ visible = true, focused = true } = {}) {
  const src = fs.readFileSync(path.join(__dirname, 'presence.js'), 'utf8');
  const listeners = { doc: {}, body: {}, win: {} };
  const on = (bag) => (type, fn) => { (bag[type] = bag[type] || []).push(fn); };
  const sent = [];
  const clock = { now: 1_000_000 };
  const state = { visible, focused };
  const window = {
    addEventListener: on(listeners.win),
    __lcWS: { send: (s) => sent.push(JSON.parse(s).type) },
  };
  const document = {
    addEventListener: on(listeners.doc),
    body: { addEventListener: on(listeners.body) },
    get visibilityState() { return state.visible ? 'visible' : 'hidden'; },
    hasFocus: () => state.focused,
  };
  const DateShim = { now: () => clock.now };
  vm.runInNewContext(src, { window, document, Date: DateShim, JSON });
  const fire = (bag, type, evt) => (listeners[bag][type] || []).forEach((fn) => fn(evt || {}));
  return { window, sent, clock, state, fire };
}

test('input sends one activity frame per minute', () => {
  const p = load();
  p.fire('doc', 'keydown');
  p.fire('doc', 'mousemove');
  p.fire('doc', 'pointerdown');
  assert.deepEqual(p.sent, ['activity']);
  p.clock.now += 59_000;
  p.fire('doc', 'keydown');
  assert.deepEqual(p.sent, ['activity']);
  p.clock.now += 1_000;
  p.fire('doc', 'keydown');
  assert.deepEqual(p.sent, ['activity', 'activity']);
});

test('a hidden page sends no activity', () => {
  const p = load({ visible: false });
  p.fire('doc', 'keydown');
  p.fire('body', 'htmx:wsOpen');
  assert.deepEqual(p.sent, []);
  p.state.visible = true;
  p.fire('doc', 'visibilitychange');
  assert.deepEqual(p.sent, ['activity']);
});

test('socket open counts only when the page is visible and focused', () => {
  const unfocused = load({ focused: false });
  unfocused.fire('body', 'htmx:wsOpen');
  assert.deepEqual(unfocused.sent, [], 'a background reconnect is not activity');

  const p = load();
  p.fire('doc', 'keydown');
  p.fire('body', 'htmx:wsOpen');
  assert.deepEqual(p.sent, ['activity', 'activity'], 'open bypasses the throttle');
});

test('a server ping is answered with pong, other frames are not', () => {
  const p = load();
  p.fire('body', 'htmx:wsAfterMessage', { detail: { message: '<!-- ping -->' } });
  p.fire('body', 'htmx:wsAfterMessage', { detail: { message: '<div id="x"></div>' } });
  assert.deepEqual(p.sent, ['pong']);
});

test('shouldSend gate', () => {
  const { shouldSend, THROTTLE_MS } = load().window.LetsChatPresence;
  assert.equal(shouldSend(5, 0, true), true);
  assert.equal(shouldSend(THROTTLE_MS, 1, true), false);
  assert.equal(shouldSend(THROTTLE_MS + 1, 1, true), true);
  assert.equal(shouldSend(THROTTLE_MS + 1, 1, false), false);
});
