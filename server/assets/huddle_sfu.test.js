// LC-840: run with `just test-js` (node --test). huddle_sfu.js is a browser
// IIFE; it is evaluated in a VM sandbox with a fake LiveKit SDK on
// window.LivekitClient (so loadSdk() resolves without a script tag) and a fake
// token endpoint. Pins the one thing the SFU session must get right about
// disconnects: a disconnect the server or network caused reaches voice.js
// through hooks.onDisconnected, and a disconnect stop() asked for does not.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'huddle_sfu.js'), 'utf8');
  const rooms = [];
  function Room() {
    this.handlers = {};
    this.remoteParticipants = new Map();
    // Fake local publications: attach() records which element the self tile
    // was bound to, so a test can assert the right local video reached it.
    const attaches = [];
    const camTrack = { attach: (el) => { attaches.push(['camera', el]); } };
    const screenTrack = { attach: (el) => { attaches.push(['screen', el]); } };
    this.localParticipant = {
      attaches,
      setMicrophoneEnabled: async () => {},
      isMicrophoneEnabled: true,
      _cam: false,
      isCameraEnabled: false,
      setCameraEnabled: async function (on) { this._cam = on; this.isCameraEnabled = on; },
      setScreenShareEnabled: async () => {},
      getTrackPublication: (src) =>
        src === 'camera' ? { videoTrack: camTrack } : src === 'screenshare' ? { videoTrack: screenTrack } : null,
    };
    this.on = (ev, fn) => { this.handlers[ev] = fn; };
    this.connect = async () => {};
    // LiveKit fires Disconnected for a client-initiated disconnect too.
    this.disconnect = async () => { if (this.handlers.Disconnected) this.handlers.Disconnected('CLIENT_INITIATED'); };
    rooms.push(this);
  }
  const LK = {
    Room,
    RoomEvent: {
      ParticipantConnected: 'ParticipantConnected',
      ParticipantDisconnected: 'ParticipantDisconnected',
      TrackSubscribed: 'TrackSubscribed',
      TrackUnsubscribed: 'TrackUnsubscribed',
      ActiveSpeakersChanged: 'ActiveSpeakersChanged',
      TrackMuted: 'TrackMuted',
      TrackUnmuted: 'TrackUnmuted',
      LocalTrackUnpublished: 'LocalTrackUnpublished',
      Disconnected: 'Disconnected',
    },
    Track: { Source: { Microphone: 'microphone', Camera: 'camera', ScreenShare: 'screenshare' } },
  };
  const sink = new El('div');
  sink.setAttribute('id', 'lc-huddle-sfu-audio-sink');
  const window = { LivekitClient: LK, console };
  const document = {
    documentElement: { getAttribute: () => 'test' },
    getElementById: (id) => (id === 'lc-huddle-sfu-audio-sink' ? sink : null),
    createElement: (t) => new El(t),
  };
  const sandbox = {
    window,
    document,
    fetch: async () => ({ ok: true, status: 200, json: async () => ({ url: 'wss://sfu', token: 't', can_publish: true }) }),
    setTimeout,
    console,
  };
  vm.runInNewContext(src, sandbox);
  return { sfu: window.LetsChatHuddleSfu, rooms };
}

function hooks() {
  const calls = [];
  return {
    calls,
    selfId: 'me',
    audioSink: () => new El('div'),
    addTile: () => {},
    removeTile: () => {},
    tileVideo: () => null,
    setHasVideo: () => {},
    onDisconnected: (reason) => { calls.push(reason); },
  };
}

// A hooks object that records the self tile's video state, with a real element
// for tileVideo(self) so attachSelfVideo has something to attach to.
function videoHooks() {
  const selfVideo = new El('video');
  const hasVideo = [];
  return {
    selfVideo,
    hasVideo,
    selfId: 'me',
    audioSink: () => new El('div'),
    addTile: () => {},
    removeTile: () => {},
    tileVideo: (uid) => (uid === 'me' ? selfVideo : null),
    setHasVideo: (uid, on) => { if (uid === 'me') hasVideo.push(on); },
    setScreen: () => {},
    setMuted: () => {},
    onDisconnected: () => {},
  };
}

test('camera on renders the local camera on the self tile', async () => {
  const t = load();
  const h = videoHooks();
  await t.sfu.start({ roomId: 7 }, h);

  const on = await t.sfu.toggleCamera();

  assert.equal(on, true);
  assert.deepEqual(t.rooms[0].localParticipant.attaches, [['camera', h.selfVideo]]);
  assert.deepEqual(h.hasVideo, [true], 'the self tile is shown as video, not a compact audio chip');
});

test('screen share renders the local share and wins over a live camera', async () => {
  const t = load();
  const h = videoHooks();
  await t.sfu.start({ roomId: 7 }, h);
  await t.sfu.toggleCamera(); // camera already live
  t.rooms[0].localParticipant.attaches.length = 0;
  h.hasVideo.length = 0;

  await t.sfu.toggleScreen();

  assert.deepEqual(t.rooms[0].localParticipant.attaches, [['screen', h.selfVideo]],
    'the screen preview replaces the camera on the self tile');
  assert.deepEqual(h.hasVideo, [true]);
});

test('ending a screen share falls back to the still-live camera', async () => {
  const t = load();
  const h = videoHooks();
  await t.sfu.start({ roomId: 7 }, h);
  await t.sfu.toggleCamera();
  await t.sfu.toggleScreen();
  t.rooms[0].localParticipant.attaches.length = 0;
  h.hasVideo.length = 0;

  // The browser's own "Stop sharing" chrome ends the track outside toggleScreen.
  t.rooms[0].handlers.LocalTrackUnpublished({ source: 'screenshare' });

  assert.deepEqual(t.rooms[0].localParticipant.attaches, [['camera', h.selfVideo]],
    'the camera preview comes back');
  assert.deepEqual(h.hasVideo, [true]);
});

test('LC-840: a disconnect the client did not ask for reaches voice.js and drops the session', async () => {
  const t = load();
  const h = hooks();
  assert.equal(await t.sfu.start({ roomId: 7 }, h), true);
  assert.equal(t.sfu.active(), true);

  t.rooms[0].handlers.Disconnected('SERVER_SHUTDOWN');

  assert.deepEqual(h.calls, ['SERVER_SHUTDOWN']);
  assert.equal(t.sfu.active(), false, 'the session is gone');
  // The leave that voice.js runs in response calls stop(): a no-op now, and
  // no second hook.
  await t.sfu.stop();
  assert.deepEqual(h.calls, ['SERVER_SHUTDOWN']);
});

test('LC-840: a disconnect stop() asked for is not reported back', async () => {
  const t = load();
  const h = hooks();
  await t.sfu.start({ roomId: 7 }, h);

  await t.sfu.stop();

  assert.equal(t.sfu.active(), false);
  assert.deepEqual(h.calls, [], 'the user left; voice.js already knows');
});

test('LC-840: a second session is not confused by the first room disconnecting late', async () => {
  const t = load();
  const first = hooks();
  await t.sfu.start({ roomId: 7 }, first);
  await t.sfu.stop();
  const second = hooks();
  await t.sfu.start({ roomId: 8 }, second);

  t.rooms[0].handlers.Disconnected('SERVER_SHUTDOWN'); // the old room, late

  assert.deepEqual(first.calls, []);
  assert.deepEqual(second.calls, []);
  assert.equal(t.sfu.active(), true, 'the live session is untouched');
});
