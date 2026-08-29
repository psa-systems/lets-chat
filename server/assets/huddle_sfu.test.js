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
    this.localParticipant = { setMicrophoneEnabled: async () => {}, isMicrophoneEnabled: true };
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
      Disconnected: 'Disconnected',
    },
    Track: { Source: { Microphone: 'microphone' } },
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
