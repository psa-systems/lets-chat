// LC-512: stage audio over a LiveKit SFU. The LC-494 control plane (roles +
// request-to-speak) is server-rendered; this connects the *media* for whoever
// is on the stage. It reconciles against the stage panel's data-lc-stage-*
// state on every htmx settle (the panel is re-rendered per viewer on every
// roster change), so joining/leaving and promote/demote drive connect /
// disconnect / re-permission automatically.
//
// The LiveKit browser SDK is loaded lazily, same-origin, from the build-vendored
// /assets/vendor/livekit-client.umd.min.js (no CDN, no CSP change; wss is
// already allowed by connect-src). If the SDK or the server config is absent,
// this no-ops and the control plane still works.
(function () {
  'use strict';

  var SDK_URL = '/assets/vendor/livekit-client.umd.min.js';
  var sdkPromise = null;
  // Live connection: { roomId, room, canPublish } or null.
  var current = null;
  var connecting = false;

  function loadSdk() {
    if (window.LivekitClient) return Promise.resolve(window.LivekitClient);
    if (sdkPromise) return sdkPromise;
    sdkPromise = new Promise(function (resolve, reject) {
      var s = document.createElement('script');
      s.src = SDK_URL;
      s.async = true;
      s.onload = function () { resolve(window.LivekitClient); };
      s.onerror = function () { reject(new Error('livekit sdk failed to load')); };
      document.head.appendChild(s);
    });
    return sdkPromise;
  }

  function audioSink() {
    var el = document.getElementById('lc-stage-audio-sink');
    if (!el) {
      el = document.createElement('div');
      el.id = 'lc-stage-audio-sink';
      el.style.display = 'none';
      document.body.appendChild(el);
    }
    return el;
  }

  async function fetchToken(roomId) {
    var res = await fetch('/room/' + roomId + '/stage/token', {
      headers: { 'Accept': 'application/json' },
    });
    if (!res.ok) throw new Error('token ' + res.status);
    return res.json(); // { url, token, can_publish }
  }

  async function connect(roomId) {
    if (connecting) return;
    connecting = true;
    try {
      var LK = await loadSdk();
      var info = await fetchToken(roomId);
      // A late state change may have already torn us down / moved rooms.
      if (current && current.roomId !== roomId) await teardown();
      var room = new LK.Room({ adaptiveStream: true, dynacast: true });
      room.on(LK.RoomEvent.TrackSubscribed, function (track) {
        if (track.kind === 'audio') {
          var el = track.attach();
          el.autoplay = true;
          audioSink().appendChild(el);
        }
      });
      room.on(LK.RoomEvent.TrackUnsubscribed, function (track) {
        track.detach().forEach(function (el) { el.remove(); });
      });
      await room.connect(info.url, info.token);
      if (info.can_publish) {
        try { await room.localParticipant.setMicrophoneEnabled(true); } catch (e) { /* mic denied */ }
      }
      current = { roomId: roomId, room: room, canPublish: !!info.can_publish };
    } catch (e) {
      // SDK missing / server unconfigured / mic blocked: leave the control
      // plane working without audio.
      if (window.console) console.warn('stage audio:', e && e.message);
    } finally {
      connecting = false;
    }
  }

  async function teardown() {
    var c = current;
    current = null;
    if (c && c.room) {
      try { await c.room.disconnect(); } catch (e) { /* already gone */ }
    }
    var sink = document.getElementById('lc-stage-audio-sink');
    if (sink) sink.replaceChildren();
  }

  // Reconcile the live connection with the panel's current per-viewer state.
  function reconcile() {
    var panel = document.querySelector('[data-lc-stage]');
    // No stage panel on this page (or stage off): ensure we are disconnected.
    if (!panel || panel.getAttribute('data-lc-stage-livekit') !== '1') {
      if (current) teardown();
      return;
    }
    var roomId = parseInt(panel.getAttribute('data-room-id'), 10);
    var joined = panel.getAttribute('data-lc-stage-joined') === '1';
    var speaker = panel.getAttribute('data-lc-stage-speaker') === '1';
    if (isNaN(roomId)) return;

    if (!joined) {
      if (current) teardown();
      return;
    }
    if (!current) {
      connect(roomId);
      return;
    }
    // Connected: a room change or a publish-permission change (promote/demote)
    // needs a fresh token, so reconnect.
    if (current.roomId !== roomId || current.canPublish !== speaker) {
      teardown().then(function () { connect(roomId); });
    }
  }

  document.body.addEventListener('htmx:afterSettle', reconcile);
  document.addEventListener('DOMContentLoaded', reconcile);
  // Drop the connection cleanly when the tab goes away.
  window.addEventListener('pagehide', function () { teardown(); });
})();
