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

function load(voiceJoined, opts) {
  opts = opts || {};
  const src = fs.readFileSync(path.join(__dirname, 'call.js'), 'utf8');
  const root = new El('div');
  root.setAttribute('id', 'lc-call-root');
  root.setAttribute('data-self-id', 'me');
  // LC-1064: minimal incoming-call dialog so handleSignal's show()/hide() and
  // the name fill have something to touch; starts hidden like the real markup.
  const incoming = new El('div');
  incoming.setAttribute('data-lc-call-incoming', '');
  incoming.classList.add('hidden');
  const incomingName = new El('div');
  incomingName.setAttribute('data-lc-call-incoming-name', '');
  incoming.appendChild(incomingName);
  root.appendChild(incoming);
  const body = new El('body');
  body.appendChild(root);
  let controls = {};
  let signalHandler = null;
  const wsSends = [];
  const notifies = [];
  const voiceCalls = [];
  const pending = () => new Promise(() => {});
  const window = {
    LetsChatRtc: {
      bindControls: (map) => { controls = map; },
      watchBus: (bus, _attr, fn) => { if (bus === 'lc-call-bus' && typeof fn === 'function') signalHandler = fn; },
      control: {
        events: { START: 'lc:control-start', INPUT: 'lc:control-input', END: 'lc:control-end', KILL: 'lc:control-kill' },
      },
    },
    LetsChatVoice: {
      isJoined: () => voiceJoined,
      leave: () => { voiceCalls.push('leave'); voiceJoined = false; },
    },
    LetsChatDevices: { getUserMedia: opts.getUserMedia || pending, applySpeaker: () => {} },
    __lcS: (k, fb) => fb,
    __lcWS: { send: (s) => { try { wsSends.push(JSON.parse(s)); } catch (e) { wsSends.push(s); } } },
    __lcNotify: (kind, msg) => notifies.push({ kind, msg }),
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
    navigator: { mediaDevices: { getDisplayMedia: opts.getDisplayMedia || pending } },
    fetch: opts.fetch || pending,
    MutationObserver: class { observe() {} disconnect() {} },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init && init.detail; } },
    alert: () => {},
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
  };
  if (opts.RTCPeerConnection) sandbox.RTCPeerConnection = opts.RTCPeerConnection;
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
  // Feed an inbound call_signal to the captured bus handler.
  const signal = (kind, attrs) => {
    if (!signalHandler) throw new Error('lc-call-bus handler was not captured');
    const n = new El('div');
    n.setAttribute('data-kind', kind);
    n.setAttribute('data-room-id', (attrs && attrs.roomId) || '42');
    n.setAttribute('data-from-id', (attrs && attrs.fromId) || 'peer');
    n.setAttribute('data-from-name', (attrs && attrs.fromName) || 'Peer');
    if (attrs && attrs.payload != null) n.setAttribute('data-payload', attrs.payload);
    signalHandler(n);
  };
  const incomingShown = () => !incoming.classList.contains('hidden');
  return {
    start, voiceCalls, signal, wsSends, notifies, incomingShown,
    win: window,
    screenClick: () => controls['[data-lc-call-screen]'](),
  };
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

test('LC-1064: an invite arriving while in a voice call auto-declines busy and does not ring', () => {
  const h = load(true); // already joined to a voice channel / huddle
  h.signal('invite', { payload: 'audio' });
  const busy = h.wsSends.filter((m) => m && m.kind === 'reject' && m.payload === 'busy');
  assert.equal(busy.length, 1, 'a busy reject is sent to the caller');
  assert.equal(busy[0].room_id, 42, 'the reject targets the inviting DM room');
  assert.equal(h.incomingShown(), false, 'the full-screen incoming panel never shows');
  assert.equal(h.notifies.length, 1, 'a quiet missed-call toast is surfaced');
});

test('LC-1064: an invite while idle still rings and sends no busy decline', () => {
  const h = load(false); // not in any voice call
  h.signal('invite', { payload: 'audio' });
  assert.equal(
    h.wsSends.filter((m) => m && m.kind === 'reject').length,
    0,
    'an idle recipient does not auto-decline',
  );
  assert.equal(h.incomingShown(), true, 'the incoming panel rings');
});

// LC-1173: a getDisplayMedia failure other than the user cancelling the
// picker (e.g. missing GStreamer/PipeWire packages on Linux) used to only
// console.warn, so the share button just did nothing with no clue why.
test('LC-1173: a getDisplayMedia failure other than cancel notifies the call UI', async () => {
  const tick = () => new Promise((r) => setTimeout(r, 0));
  const h = load(false, {
    getUserMedia: () => Promise.resolve({
      getTracks: () => [],
      getAudioTracks: () => [],
      getVideoTracks: () => [],
    }),
    fetch: () => Promise.reject(new Error('no ice config')),
    RTCPeerConnection: class { addTrack() {} },
    getDisplayMedia: () => Promise.reject(Object.assign(new Error('not found'), { name: 'NotFoundError' })),
  });
  h.start(false);           // becomes the outgoing caller once media resolves
  await tick(); await tick();
  h.signal('accept');       // peer answers: onAccept() creates the pc
  await tick();

  h.screenClick();
  await tick(); await tick();

  const errs = h.notifies.filter((n) => n.kind === 'err');
  assert.equal(errs.length, 1, 'exactly one error notification');
  assert.match(errs[0].msg, /Screen sharing failed/);
});

// LC-1173: the user dismissing the picker (NotAllowedError/AbortError) is not
// a fault - no notification for that case.
test('LC-1173: cancelling the screen picker does not notify', async () => {
  const tick = () => new Promise((r) => setTimeout(r, 0));
  const h = load(false, {
    getUserMedia: () => Promise.resolve({
      getTracks: () => [],
      getAudioTracks: () => [],
      getVideoTracks: () => [],
    }),
    fetch: () => Promise.reject(new Error('no ice config')),
    RTCPeerConnection: class { addTrack() {} },
    getDisplayMedia: () => Promise.reject(Object.assign(new Error('cancelled'), { name: 'NotAllowedError' })),
  });
  h.start(false);
  await tick(); await tick();
  h.signal('accept');
  await tick();

  h.screenClick();
  await tick(); await tick();

  assert.equal(h.notifies.length, 0, 'a user cancel is not a fault');
});
