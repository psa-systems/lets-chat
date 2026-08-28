// LC-832: run with `just test-js` (node --test). Drives the real voice.js and
// the real huddle_popout.js together in one VM sandbox over the shared DOM stub
// (test_dom.js), so what is asserted is the seam between them rather than a
// mock of it. The SFU huddle is the join path used here: joinSfu() hands media
// to huddle_sfu.js (stubbed), so a call can be joined and left without
// getUserMedia or RTCPeerConnection, while sharing every presence/UI/teardown
// step with the mesh path.
//
// Covered: which branch scan() takes on a swap (float, leave-for-a-different-
// dock, plain unbind), that leave() takes the floating panel down with it (the
// Leave button, and call.js accepting a 1:1 DM call), and that the teardown is
// a real one - `voice_leave` on the wire and the SFU session stopped.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

// The room huddle dock as room/huddle.html renders it. `sfu` picks the join
// path; the enclave voice channel page (voice/page.html) differs only in the
// attributes it omits, which voice.js reads as mesh.
function makeDock(roomId, sfu) {
  const root = new El('div');
  root.setAttribute('data-lc-voice-root', '');
  root.setAttribute('data-lc-huddle', '');
  root.setAttribute('data-room-id', String(roomId));
  root.setAttribute('data-self-id', 'u1');
  root.setAttribute('data-self-name', 'Alice');
  root.setAttribute('data-ice-servers', '[]');
  root.setAttribute('data-lc-huddle-sfu', sfu === false ? '0' : '1');
  root.className = 'lc-huddle';
  const bar = new El('div');
  bar.className = 'lc-huddle-bar';
  const join = new El('button');
  join.setAttribute('data-lc-voice-join', '');
  bar.appendChild(join);
  root.appendChild(bar);
  root.appendChild(new El('video'));
  return root;
}

// The enclave voice channel page root (voice/page.html): the same
// [data-lc-voice-root] contract minus the huddle opt-ins, and no SFU attribute,
// which voice.js reads as mesh. Its bar is .lc-callbar, not .lc-huddle-bar.
function makeVoicePage(roomId) {
  const root = new El('div');
  root.setAttribute('data-lc-voice-root', '');
  root.setAttribute('data-room-id', String(roomId));
  root.setAttribute('data-self-id', 'u1');
  root.setAttribute('data-self-name', 'Alice');
  root.setAttribute('data-ice-servers', '[]');
  const bar = new El('header');
  bar.className = 'lc-callbar';
  root.appendChild(bar);
  root.appendChild(new El('video'));
  return root;
}

// One sandbox holding both modules, with the page structure that matters here:
// the swapped <main id="main"> plus the persistent body a floated dock is
// re-parented into. Document order is real - the page's own dock is inside
// #main and so precedes a floating panel appended to the body.
function load() {
  const body = new El('body');
  const main = new El('main');
  main.setAttribute('id', 'main');
  body.appendChild(main);
  const bodyListeners = {};
  const sent = [];
  const sfuCalls = [];
  const tracks = [{ kind: 'audio', enabled: true, stopped: false, stop() { this.stopped = true; } }];
  const store = new Map();
  let controls = {};

  const document = {
    readyState: 'complete',
    body,
    documentElement: { attributes: [] },
    styleSheets: [],
    createElement: (t) => new El(t),
    getElementById: () => null,
    addEventListener: () => {},
    dispatchEvent: () => true,
    querySelector: (sel) => body.querySelector(sel),
    querySelectorAll: (sel) => body.querySelectorAll(sel),
  };
  body.addEventListener = (type, fn) => { (bodyListeners[type] = bodyListeners[type] || []).push(fn); };

  const window = {
    innerWidth: 1280,
    innerHeight: 800,
    __lcS: (k, fb) => fb,
    location: { pathname: '/room/7', search: '', hash: '' },
    history: { replaceState: () => {} },
    // LC-616: voice.js takes its control-bar dispatch and event bus from here.
    LetsChatRtc: {
      bindControls: (map) => { controls = map; },
      watchBus: () => {},
    },
    // LC-610: the SFU owns media on this path.
    LetsChatHuddleSfu: {
      start: () => { sfuCalls.push('start'); return Promise.resolve(true); },
      stop: () => { sfuCalls.push('stop'); },
    },
    // LC-144: the mesh path takes its mic through the pinned-device module.
    LetsChatDevices: {
      getUserMedia: () => Promise.resolve({
        id: 'stream-1',
        getTracks: () => tracks,
        getAudioTracks: () => tracks,
        getVideoTracks: () => [],
      }),
      applySpeaker: () => {},
    },
  };

  const sandbox = {
    window,
    document,
    localStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
    },
    console: { warn: () => {}, log: () => {}, error: () => {} },
    alert: () => {},
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    Promise,
    URLSearchParams,
    CustomEvent: class { constructor(t, o) { this.type = t; this.detail = o && o.detail; } },
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  // Load order matches base.html: voice.js, then huddle_popout.js.
  for (const f of ['voice.js', 'huddle_popout.js']) {
    vm.runInContext(fs.readFileSync(path.join(__dirname, f), 'utf8'), sandbox);
  }

  const fire = (type, detail) => {
    (bodyListeners[type] || []).forEach((fn) => fn({ type, detail, target: body }));
  };
  // A live socket, as layout.html's htmx:wsOpen handler supplies one.
  fire('htmx:wsOpen', { socketWrapper: { send: (s) => sent.push(JSON.parse(s)) } });

  return {
    body,
    main,
    sent,
    sfuCalls,
    tracks,
    fire,
    voice: window.LetsChatVoice,
    popout: window.LetsChatHuddlePopout,
    join: () => controls['[data-lc-voice-join]'](),
    leaveClick: () => controls['[data-lc-voice-leave]'](),
    // htmx swapped #main: the old dock is gone from the document, and whatever
    // the new page rendered is in its place. A floating panel lives on the body,
    // outside #main, so the swap does not touch it.
    swap: (next) => {
      main.replaceChildren();
      if (next) main.appendChild(next);
      fire('htmx:afterSettle');
    },
  };
}

// A joined call on room 7's dock, as the page it was joined from.
function joined() {
  const h = load();
  const dock = makeDock(7);
  h.main.appendChild(dock);
  h.fire('htmx:afterSettle');           // scan() binds the page's dock
  h.join();
  assert.equal(h.voice.isJoined(), true, 'setup: the call is live');
  return Object.assign(h, { dock });
}

test('LC-832: a swap with no dock in the page floats the live one instead of leaving', () => {
  const h = joined();

  h.swap(null);                         // navigated to a page with no voice root

  assert.equal(h.voice.isJoined(), true, 'the call outlives the page it started on');
  assert.equal(h.popout.isPopped(), true);
  assert.equal(h.popout.roomId(), '7');
  assert.equal(h.dock.parentNode, h.body, 'the LIVE dock is re-parented outside #main');
  assert.equal(h.dock.classList.contains('lc-huddle--float'), true);
  assert.deepEqual(h.sfuCalls, ['start'], 'no teardown ran');
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 0);
});

test('LC-832: the floated dock survives further swaps, and its own room shows the bring-back placeholder', () => {
  const h = joined();
  h.swap(null);

  // Another room with a huddle: busy note, nothing joined and nothing dropped.
  const other = makeDock(9);
  h.main.appendChild(other);
  h.fire('htmx:afterSettle');
  assert.equal(other.hasAttribute('data-lc-huddle-busy'), true);
  assert.equal(h.voice.isJoined(), true);
  assert.equal(h.popout.roomId(), '7', 'still bound to the live room');

  // Back to room 7: its re-rendered dock becomes the bring-back placeholder.
  other.remove();
  const fresh = makeDock(7);
  h.main.appendChild(fresh);
  h.fire('htmx:afterSettle');
  assert.equal(fresh.parentNode, null, 'no second dock for the live room');
  assert.ok(h.body.querySelector('[data-lc-huddle-placeholder][data-room-id="7"]'));
  assert.equal(h.voice.isJoined(), true);

  // Bringing it back re-docks the same live element.
  h.popout.bringBack();
  assert.equal(h.popout.isPopped(), false);
  assert.equal(h.dock.parentNode, h.main, 'the live dock goes back into the page');
  assert.equal(h.dock.classList.contains('lc-huddle--float'), false);
  assert.equal(h.voice.isJoined(), true);
});

test('LC-832: leaving from the floating dock tears the call down and removes the panel', () => {
  const h = joined();
  h.swap(null);

  h.leaveClick();                       // Leave, pressed on the floating dock

  assert.equal(h.voice.isJoined(), false);
  assert.deepEqual(h.sfuCalls, ['start', 'stop'], 'media is stopped');
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1, 'the roster is told');
  assert.equal(h.popout.isPopped(), false);
  assert.equal(h.body.contains(h.dock), false, 'no orphaned panel outlives the call');
});

test('LC-832: accepting a 1:1 DM call ends the voice channel and leaves no orphaned panel', () => {
  const h = joined();
  h.swap(null);                         // reading another page, call floating

  // call.js:552-555, verbatim.
  if (h.voice && h.voice.isJoined()) h.voice.leave();

  assert.equal(h.voice.isJoined(), false);
  assert.deepEqual(h.sfuCalls, ['start', 'stop']);
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1);
  assert.equal(h.popout.isPopped(), false, 'the float is released, not left behind');
  assert.equal(h.body.contains(h.dock), false, 'no orphaned panel over the DM call');
});

test('LC-832: a DM call accepted while the dock is still docked leaves exactly as before', () => {
  const h = joined();

  if (h.voice && h.voice.isJoined()) h.voice.leave();

  assert.equal(h.voice.isJoined(), false);
  assert.deepEqual(h.sfuCalls, ['start', 'stop']);
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1);
  assert.equal(h.popout.isPopped(), false);
});

test('LC-832: the enclave voice channel page floats too, and its mesh leave releases the float', async () => {
  const h = load();
  h.main.appendChild(makeVoicePage(7));
  h.fire('htmx:afterSettle');
  h.join();
  await Promise.resolve();              // getUserMedia resolves
  await Promise.resolve();
  assert.equal(h.voice.isJoined(), true, 'setup: the mesh call is live');
  const page = h.main.childNodes[0];

  h.swap(null);

  assert.equal(h.voice.isJoined(), true, 'the voice channel outlives its page');
  assert.equal(page.parentNode, h.body);
  assert.equal(page.classList.contains('lc-huddle--float'), true);

  h.leaveClick();

  // leave()'s mesh branch is a different function body from the SFU one above:
  // it must release the float as well, and stop the local tracks.
  assert.equal(h.voice.isJoined(), false);
  assert.equal(h.popout.isPopped(), false, 'no orphaned panel on the mesh path');
  assert.equal(h.body.contains(page), false);
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1);
  assert.equal(h.tracks[0].stopped, true, 'the microphone is released');
});

test('LC-832: a swap that renders a DIFFERENT dock still ends the call (bindRoot), never floats', () => {
  const h = joined();

  h.swap(makeDock(9));                  // room 9's page, rendered by the swap

  assert.equal(h.voice.isJoined(), false, 'one voice session per tab');
  assert.deepEqual(h.sfuCalls, ['start', 'stop']);
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1);
  assert.equal(h.popout.isPopped(), false, 'nothing was floated');
  assert.equal(h.dock.classList.contains('lc-huddle--float'), false);

  // Rebound to the new page, so a Join there joins room 9.
  h.join();
  assert.equal(h.voice.isJoined(), true);
  assert.equal(h.sent.filter((f) => f.type === 'voice_join' && f.room_id === 9).length, 1);
});

test('LC-832: a swap with no dock and no live call just unbinds - nothing floats', () => {
  const h = load();
  const dock = makeDock(7);
  h.main.appendChild(dock);
  h.fire('htmx:afterSettle');           // bound, never joined

  h.swap(null);

  assert.equal(h.popout.isPopped(), false);
  assert.equal(dock.classList.contains('lc-huddle--float'), false);
  assert.equal(h.voice.isJoined(), false);
});

test('LC-832: a float that does not take falls back to leaving, never a live mic with no UI', () => {
  const h = joined();
  h.popout.floatOut = () => {};         // controller present but the lift fails

  h.swap(null);

  assert.equal(h.voice.isJoined(), false, 'the call ends rather than going invisible');
  assert.deepEqual(h.sfuCalls, ['start', 'stop']);
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1);
});
