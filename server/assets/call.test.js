// LC-839: run with `just test-js` (node --test). call.js is a browser IIFE;
// it is evaluated in a VM sandbox over the shared DOM stub with just enough of
// the page for the outgoing-call control to run up to media acquisition, which
// is stubbed to never settle. Pins that placing an outgoing DM call leaves a
// joined voice channel or huddle first, the way accepting one already did.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

function load(voiceJoined) {
  const src = fs.readFileSync(path.join(__dirname, 'call.js'), 'utf8');
  const root = new El('div');
  root.setAttribute('id', 'lc-call-root');
  root.setAttribute('data-self-id', 'me');
  const body = new El('body');
  body.appendChild(root);
  let controls = {};
  const voiceCalls = [];
  const pending = () => new Promise(() => {});
  const window = {
    LetsChatRtc: { bindControls: (map) => { controls = map; }, watchBus: () => {} },
    LetsChatVoice: {
      isJoined: () => voiceJoined,
      leave: () => { voiceCalls.push('leave'); voiceJoined = false; },
    },
    LetsChatDevices: { getUserMedia: pending },
    __lcS: (k, fb) => fb,
    location: { pathname: '/dm/peer', search: '', hash: '', origin: 'https://x' },
    addEventListener: () => {},
    matchMedia: () => ({ matches: false, addEventListener: () => {} }),
    setTimeout,
    clearTimeout,
  };
  const document = {
    readyState: 'complete',
    body,
    documentElement: { getAttribute: () => 'test', attributes: [] },
    getElementById: (id) => (id === 'lc-call-root' ? root : null),
    querySelector: (sel) => body.querySelector(sel),
    querySelectorAll: (sel) => body.querySelectorAll(sel),
    createElement: (t) => new El(t),
    addEventListener: () => {},
    dispatchEvent: () => true,
  };
  const sandbox = {
    window,
    document,
    navigator: { mediaDevices: {} },
    fetch: pending,
    MutationObserver: class { observe() {} disconnect() {} },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init && init.detail; } },
    alert: () => {},
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
  };
  sandbox.self = window;
  vm.runInNewContext(src, sandbox);
  const start = (video) => {
    const btn = new El('button');
    btn.setAttribute('data-lc-call-start', video ? 'video' : 'audio');
    btn.setAttribute('data-room-id', '42');
    btn.setAttribute('data-peer-id', 'peer');
    btn.setAttribute('data-peer-name', 'Peer');
    controls['[data-lc-call-start]'](btn, { preventDefault() {} });
  };
  return { start, voiceCalls };
}

test('LC-839: placing an outgoing call leaves a joined voice channel or huddle first', () => {
  const h = load(true);
  h.start(false);
  assert.deepEqual(h.voiceCalls, ['leave'], 'the voice session is left before media is acquired');
});

test('LC-839: placing a call with no voice session leaves nothing', () => {
  const h = load(false);
  h.start(true);
  assert.deepEqual(h.voiceCalls, []);
});
