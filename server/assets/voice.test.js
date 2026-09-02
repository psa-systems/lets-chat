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
  // The tile grid: createTile / updateTileMedia render into this, so a test
  // that inspects the self tile needs it present.
  const grid = new El('div');
  grid.setAttribute('data-lc-voice-grid', '');
  root.appendChild(grid);
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
  const toasts = [];
  let sfuHooks = null;
  let busCb = null;
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
      // Capture the inbound-event callback so a test can drive server frames
      // (roster/joined/screen/...) the way the real bus delivers them.
      watchBus: (_bus, _attr, cb) => { busCb = cb; },
    },
    // LC-610: the SFU owns media on this path.
    LetsChatHuddleSfu: {
      // LC-840: keep the hooks so a test can fire onDisconnected as the SFU would.
      start: (_cfg, hooks) => { sfuCalls.push('start'); sfuHooks = hooks; return Promise.resolve(true); },
      stop: () => { sfuCalls.push('stop'); },
    },
    __lcToast: (kind, msg) => { toasts.push([kind, msg]); },
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
    toasts,
    sfuHooks: () => sfuHooks,
    // Deliver one inbound server frame as a bus node (data-* attributes), the
    // way LetsChatRtc.watchBus would hand it to voice.js's handleEvent.
    event: (attrs) => {
      const n = new El('div');
      Object.keys(attrs).forEach((k) => n.setAttribute(k, String(attrs[k])));
      if (busCb) busCb(n);
    },
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

// LC-837 made back/forward an htmx history restore of #main, which fires
// htmx:historyRestore and NOT htmx:afterSettle. scan() must run on both, or a
// restore detaches the dock without floating it (a live mic with no UI) and a
// restore back into the call room renders a second dock beside the float.
test('LC-832: a history restore that leaves no dock floats the live one, exactly like a swap', () => {
  const h = joined();

  h.main.replaceChildren();             // back/forward restored a page with no voice root
  h.fire('htmx:historyRestore');

  assert.equal(h.voice.isJoined(), true, 'the call outlives the restore');
  assert.equal(h.popout.isPopped(), true, 'the dock floated on htmx:historyRestore');
  assert.equal(h.dock.parentNode, h.body, 'the LIVE dock is re-parented outside #main');
  assert.deepEqual(h.sfuCalls, ['start'], 'no teardown ran');
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 0);
});

test('LC-832: a history restore back into the call room shows the placeholder, not a second dock', () => {
  const h = joined();
  h.swap(null);
  assert.equal(h.popout.isPopped(), true);

  const fresh = makeDock(7);
  h.main.replaceChildren();
  h.main.appendChild(fresh);
  h.fire('htmx:historyRestore');

  assert.equal(fresh.parentNode, null, 'no second dock for the live room');
  assert.ok(h.body.querySelector('[data-lc-huddle-placeholder][data-room-id="7"]'));
  assert.equal(h.voice.isJoined(), true);
  assert.equal(h.popout.roomId(), '7');
});

// LC-840: an SFU-side disconnect the client did not ask for must end the
// huddle, or voice.js keeps `joined` true with no media.
test('LC-840: an SFU disconnect leaves the huddle, releases the float and tells the user', () => {
  const h = joined();
  h.swap(null);                         // floated, as after a navigation
  assert.equal(h.popout.isPopped(), true);

  h.sfuHooks().onDisconnected('SERVER_SHUTDOWN');

  assert.equal(h.voice.isJoined(), false);
  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, 1, 'the server roster is told');
  assert.equal(h.popout.isPopped(), false, 'the floating dock is released');
  assert.equal(h.body.querySelector('.lc-huddle--float'), null, 'no floating panel remains');
  assert.equal(h.toasts.length, 1, 'the user is told');
  assert.equal(h.toasts[0][0], 'err');
});

test('LC-840: a disconnect after the user already left runs nothing twice', () => {
  const h = joined();
  h.leaveClick();
  assert.equal(h.voice.isJoined(), false);
  const before = h.sent.filter((f) => f.type === 'voice_leave').length;

  h.sfuHooks().onDisconnected('CLIENT_INITIATED');

  assert.equal(h.sent.filter((f) => f.type === 'voice_leave').length, before, 'no second voice_leave');
  assert.equal(h.toasts.length, 0, 'nothing to tell: the user chose to leave');
});

test('LC-610: the voice_screen echo of our own share does not hide the SFU self tile', () => {
  const h = joined();
  const grid = h.dock.querySelector('[data-lc-voice-grid]');
  const tile = grid.querySelector('[data-lc-voice-tile="u1"]');
  const video = tile.querySelector('[data-lc-voice-video]');

  // huddle_sfu.js attached our live screen track and marked the tile as video.
  h.sfuHooks().setHasVideo('u1', true);
  h.sfuHooks().setScreen('u1', true);
  assert.equal(video.style.display, '', 'precondition: the share is visible');
  assert.equal(tile.getAttribute('data-media'), 'video');

  // The server echoes our own voice_screen back to us; handleEvent runs
  // updateTileMedia(self). In SFU it must NOT recompute from the null
  // localStream and hide the live share (the black-out bug).
  h.event({ 'data-room-id': 7, 'data-kind': 'screen', 'data-user-id': 'u1', 'data-payload': '1' });

  assert.equal(video.style.display, '', 'the self share stays visible after its own echo');
  assert.equal(tile.getAttribute('data-media'), 'video', 'the self tile stays a video tile');
  assert.equal(tile.getAttribute('data-screen'), 'true', 'the share pin is still set');
});

test('LC-610: the screen echo still nudges a REMOTE peer tile (the self skip is self-only)', () => {
  const h = joined();
  const grid = h.dock.querySelector('[data-lc-voice-grid]');

  // A remote peer marked as video, whose track has since gone dead (a frozen
  // last frame after they stopped sharing). Their voice_screen=false echo must
  // still run updateTileMedia for them and revert the frozen frame - the self
  // skip added for the black-out fix must not swallow the remote nudge.
  h.sfuHooks().addTile('u2', 'Bob');
  const tile = grid.querySelector('[data-lc-voice-tile="u2"]');
  const video = tile.querySelector('[data-lc-voice-video]');
  h.sfuHooks().setHasVideo('u2', true);
  video.srcObject = { getVideoTracks: () => [{ readyState: 'ended', muted: false }] };
  assert.equal(tile.getAttribute('data-media'), 'video', 'precondition: shown as video');

  h.event({ 'data-room-id': 7, 'data-kind': 'screen', 'data-user-id': 'u2', 'data-payload': '0' });

  assert.equal(tile.getAttribute('data-media'), 'audio', 'the remote peer nudge still fires');
});
