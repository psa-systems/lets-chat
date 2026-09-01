// LC-610: huddle media over a LiveKit SFU.
//
// A huddle's transport is fixed by server config: when LiveKit is configured
// (`data-lc-huddle-sfu="1"` on the voice root) a huddle runs entirely over the
// SFU; otherwise it runs entirely over the WebRTC mesh in voice.js, unchanged.
// There is no mid-call switch - see the SFU_MIN_PARTICIPANTS note in
// livekit.rs for why the threshold model was dropped.
//
// voice.js owns the tiles, the control buttons, and the WS presence
// (`voice_join`/`voice_leave`, which the token endpoint gates on). This module
// owns only the LiveKit connection and the media: it publishes the local mic
// and camera, subscribes to remote tracks, and renders each into voice.js's
// existing tile grid via the small hook API voice.js hands it in `start()`. So
// an SFU huddle looks identical to a mesh one; only the transport differs.
//
// The LiveKit SDK is loaded lazily, same-origin, from the build-vendored bundle
// (no CDN, no CSP change; wss is already allowed by connect-src). If the SDK or
// the token is unavailable the call reports failure and voice.js is expected to
// have already chosen the mesh, so nothing here needs to fall back.
(function () {
  'use strict';

  // LC-776: /assets/* answers a `?v=` URL with a one-year immutable
  // Cache-Control, so a URL built here has to carry the page's asset version
  // or a rebuilt bundle is never picked up. base.html publishes it on <html>.
  var ASSET_V = document.documentElement.getAttribute('data-lc-asset-version') || 'dev';
  var SDK_URL = '/assets/vendor/livekit-client.umd.min.js?v=' + encodeURIComponent(ASSET_V);
  var sdkPromise = null;

  // Live session, or null. hooks are voice.js's tile callbacks.
  var session = null;

  // LC-854: huddle_control.js registers a callback here to receive inbound
  // remote-control frames (it is the controlled side). One at a time; cleared
  // when control ends. cb(fromIdentity, text).
  var controlDataCb = null;

  // LC-764: opt-in call diagnostics. Set `window.__lcCallDebug = true` in the
  // console before reproducing to log the local mic track state on both sides of
  // a mute toggle - the state acceptance criterion one asks to record. Off by
  // default, so a normal call logs nothing.
  function dbg() {
    if (!window.__lcCallDebug || !window.console || !console.log) return;
    try { console.log.apply(console, ['[lc-call sfu]'].concat([].slice.call(arguments))); } catch (e) {}
  }

  // Read the published microphone track's real state for diagnostics. Defensive
  // across LiveKit versions: never throws, returns whatever it can resolve.
  function micTrackState(lp) {
    var out = { micEnabled: lp && lp.isMicrophoneEnabled };
    try {
      var LK = window.LivekitClient;
      var pub = LK && LK.Track && lp.getTrackPublication
        ? lp.getTrackPublication(LK.Track.Source.Microphone)
        : null;
      var mst = pub && pub.track && pub.track.mediaStreamTrack;
      if (pub) out.pubMuted = pub.isMuted;
      if (mst) { out.readyState = mst.readyState; out.trackMuted = mst.muted; out.trackEnabled = mst.enabled; }
    } catch (e) { out.err = e && e.message; }
    return out;
  }

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

  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }

  // The token endpoint gates on hub membership, which voice.js establishes with
  // a `voice_join` WS frame sent just before this runs. WS and HTTP are separate
  // connections, so the frame may not be processed when the first request lands
  // (a 400 "Join the huddle before connecting media"). Retry a few times to
  // absorb that ordering; a persistent 400 is a real refusal (unconfigured, not
  // a member) and gives up.
  async function fetchToken(roomId) {
    var delays = [0, 150, 300, 600];
    var lastStatus = 0;
    for (var i = 0; i < delays.length; i++) {
      if (delays[i]) await sleep(delays[i]);
      var res = await fetch('/room/' + encodeURIComponent(roomId) + '/huddle/token', {
        headers: { 'Accept': 'application/json' },
      });
      if (res.ok) return res.json(); // { url, token, can_publish }
      lastStatus = res.status;
      // Only a 400 is worth retrying (membership may not be registered yet).
      // Anything else (403 access, 404 room) is terminal.
      if (res.status !== 400) break;
    }
    throw new Error('token ' + lastStatus);
  }

  // Identity of a LiveKit participant is the lets-chat user id (mint_token's
  // `sub`), so tiles key on the same id the mesh path uses.
  function idOf(participant) { return participant && participant.identity; }

  // LC-814: the server may dispatch a hidden transcription agent into the SFU
  // room (see the LC-810 design). It joins only to subscribe to audio and must
  // never appear in the roster, so identities under the reserved `agent-` prefix
  // (livekit::AGENT_IDENTITY_PREFIX) get no tile. Kept distinct from user ids.
  function isAgent(id) { return typeof id === 'string' && id.indexOf('agent-') === 0; }

  function attachTrack(hooks, participant, track) {
    var uid = idOf(participant);
    if (!uid) return;
    if (track.kind === 'audio') {
      // Audio is never shown; attach into the hidden sink so it plays. The
      // self participant's own audio is not subscribed by LiveKit, so this only
      // ever handles remote audio - no echo.
      var el = track.attach();
      el.autoplay = true;
      hooks.audioSink().appendChild(el);
      return;
    }
    // Video (camera or screen) goes onto the participant's tile.
    var videoEl = hooks.tileVideo(uid);
    if (videoEl) {
      track.attach(videoEl);
      hooks.setHasVideo(uid, true);
    }
  }

  function detachTrack(hooks, participant, track) {
    var uid = idOf(participant);
    track.detach().forEach(function (el) { el.remove(); });
    if (track.kind === 'video' && uid) hooks.setHasVideo(uid, false);
  }

  async function start(cfg, hooks) {
    if (session) return true;
    var LK, info;
    try {
      LK = await loadSdk();
      info = await fetchToken(cfg.roomId);
    } catch (e) {
      // No SDK or no token: voice.js chose this path from data-lc-huddle-sfu,
      // so a failure here means audio simply does not come up. Report it so the
      // caller can surface the mic/connection error rather than silently
      // pretending the user is in a working call.
      if (window.console) console.warn('huddle sfu:', e && e.message);
      return false;
    }

    var room = new LK.Room({ adaptiveStream: true, dynacast: true });
    session = { roomId: cfg.roomId, room: room, hooks: hooks, sharing: false };

    // LC-840: the SFU ended the session under us (server shutdown or restart,
    // the room closed, this participant removed, the network past LiveKit's
    // reconnect window). stop() clears `session` BEFORE it disconnects, so a
    // disconnect we asked for finds a different (or no) session here and is
    // ignored; the connect-failure path below does the same. Anything else
    // drops the session and tells voice.js, which otherwise keeps `joined`
    // true with no media: a dock still offering Leave and Mute over silence.
    var mine = session;
    room.on(LK.RoomEvent.Disconnected, function (reason) {
      if (session !== mine) return;
      session = null;
      var sink = document.getElementById('lc-huddle-sfu-audio-sink');
      if (sink) sink.replaceChildren();
      if (hooks.onDisconnected) hooks.onDisconnected(reason);
    });

    // A remote joined / left: mirror onto the tile grid so the roster matches
    // the mesh path (where VoiceJoined/VoiceLeft drive the same tiles).
    room.on(LK.RoomEvent.ParticipantConnected, function (p) {
      if (isAgent(idOf(p))) return;
      hooks.addTile(idOf(p), p.name || idOf(p));
    });
    room.on(LK.RoomEvent.ParticipantDisconnected, function (p) {
      hooks.removeTile(idOf(p));
    });
    room.on(LK.RoomEvent.TrackSubscribed, function (track, _pub, participant) {
      attachTrack(hooks, participant, track);
    });
    room.on(LK.RoomEvent.TrackUnsubscribed, function (track, _pub, participant) {
      detachTrack(hooks, participant, track);
    });
    // LiveKit's own active-speaker detection replaces the mesh's local audio
    // analysis; light the same speaking indicator on each tile.
    room.on(LK.RoomEvent.ActiveSpeakersChanged, function (speakers) {
      var loud = {};
      speakers.forEach(function (p) { loud[idOf(p)] = true; });
      hooks.setSpeaking(loud);
    });
    // LC-853: the browser's own "Stop sharing" chrome ends the screen track
    // without going through toggleScreen(); LiveKit auto-unpublishes it and
    // fires this. Sync our sharing state and tell voice.js, so the WS
    // announcement (remote tile pins + the server's sharer tracking, which
    // routes remote-control requests) follows instead of going stale.
    room.on(LK.RoomEvent.LocalTrackUnpublished, function (pub) {
      if (session !== mine) return;
      if (!pub || pub.source !== LK.Track.Source.ScreenShare) return;
      mine.sharing = false;
      if (hooks.screenEnded) hooks.screenEnded();
    });
    // LC-854: inbound remote-control data. Frames ride LiveKit's data channel
    // on the reserved `lc-control` topic, addressed to a single recipient; hand
    // each to huddle_control.js, which is the controlled side and forwards to
    // the native injector. The topic keeps this off any other data use.
    room.on(LK.RoomEvent.DataReceived, function (payload, participant, _kind, topic) {
      if (topic !== 'lc-control') return;
      if (!controlDataCb) return;
      var text;
      try { text = new TextDecoder().decode(payload); } catch (e) { return; }
      controlDataCb(participant ? idOf(participant) : null, text);
    });
    // A remote muting/unmuting their mic: reflect it on their tile.
    room.on(LK.RoomEvent.TrackMuted, function (_pub, p) {
      if (_pub.kind === 'audio') hooks.setMuted(idOf(p), true);
    });
    room.on(LK.RoomEvent.TrackUnmuted, function (_pub, p) {
      if (_pub.kind === 'audio') hooks.setMuted(idOf(p), false);
    });

    try {
      await room.connect(info.url, info.token);
    } catch (e) {
      if (window.console) console.warn('huddle sfu connect:', e && e.message);
      session = null;
      try { room.disconnect(); } catch (ignored) {}
      return false;
    }

    // Seed tiles for participants already in the room at connect time; the
    // events above only fire for later changes.
    room.remoteParticipants.forEach(function (p) {
      if (isAgent(idOf(p))) return;
      hooks.addTile(idOf(p), p.name || idOf(p));
    });

    // Publish the mic. Camera is off until the user turns it on, matching the
    // mesh path (join() there captures audio-only).
    if (info.can_publish) {
      try {
        await room.localParticipant.setMicrophoneEnabled(true);
      } catch (e) {
        // Mic denied: stay connected as a listener, same as the mesh path,
        // which also proceeds when getUserMedia fails for video but here the
        // user can still hear others.
        if (window.console) console.warn('huddle sfu mic:', e && e.message);
      }
    }
    return true;
  }

  async function stop() {
    var s = session;
    session = null;
    if (!s) return;
    try { await s.room.disconnect(); } catch (e) { /* already gone */ }
    var sink = document.getElementById('lc-huddle-sfu-audio-sink');
    if (sink) sink.replaceChildren();
  }

  function active() { return !!session; }

  // Local controls, delegated from voice.js's buttons. Each maps onto a LiveKit
  // publish toggle and returns the new state so voice.js can update the button.
  async function toggleMute() {
    if (!session) return false;
    var lp = session.room.localParticipant;
    // `isMicrophoneEnabled` is true when the mic is ON (unmuted). Read it BEFORE
    // the toggle; the post-toggle muted state is derived from the REAL reading
    // afterwards (mute-state.js), never from the state we aimed for, so a toggle
    // that rejects or no-ops cannot leave the button lying.
    var before = lp.isMicrophoneEnabled;
    dbg('toggle: before', micTrackState(lp));
    var ok = true;
    try {
      await lp.setMicrophoneEnabled(!before);
    } catch (e) {
      ok = false;
      if (window.console) console.warn('huddle sfu mic toggle:', e && e.message);
    }
    var after = lp.isMicrophoneEnabled;
    var r = window.LetsChatMute.nextState(before, after, ok);
    dbg('toggle: after', micTrackState(lp), 'failed', r.failed);
    // LC-764: a failed re-enable used to be swallowed (the button silently
    // corrected itself). Surface it so the caller knows their unmute did not
    // take and peers are not hearing them.
    if (r.failed && session.hooks.micError) session.hooks.micError();
    session.hooks.setMuted(session.hooks.selfId, r.muted);
    return r.muted; // true = now muted
  }

  async function toggleCamera() {
    if (!session) return false;
    var lp = session.room.localParticipant;
    var on = !lp.isCameraEnabled;
    try { await lp.setCameraEnabled(on); } catch (e) {
      if (window.console) console.warn('huddle sfu camera:', e && e.message);
      return lp.isCameraEnabled;
    }
    // Attach our own camera preview onto the self tile (LiveKit does not loop
    // local media back through subscription).
    var pub = lp.getTrackPublication && lp.getTrackPublication(window.LivekitClient.Track.Source.Camera);
    var videoEl = session.hooks.tileVideo(session.hooks.selfId);
    if (on && pub && pub.videoTrack && videoEl) {
      pub.videoTrack.attach(videoEl);
      session.hooks.setHasVideo(session.hooks.selfId, true);
    } else if (!on) {
      session.hooks.setHasVideo(session.hooks.selfId, false);
    }
    return on;
  }

  async function toggleScreen() {
    if (!session) return false;
    var lp = session.room.localParticipant;
    var on = !session.sharing;
    try { await lp.setScreenShareEnabled(on); } catch (e) {
      // User-cancel from the picker lands here too; nothing to undo.
      return session.sharing;
    }
    session.sharing = on;
    session.hooks.setScreen(session.hooks.selfId, on);
    return on;
  }

  // LC-854: send one remote-control frame to a single peer over the data
  // channel. `text` is the JSON frame; `toIdentity` the sharer's user id;
  // `reliable` false for movement (drop-old is fine), true for clicks/keys.
  // No-op unless a session is live and the token granted publishData.
  function sendControl(text, toIdentity, reliable) {
    if (!session || !session.room) return;
    var lp = session.room.localParticipant;
    if (!lp || !lp.publishData) return;
    var bytes;
    try { bytes = new TextEncoder().encode(text); } catch (e) { return; }
    try {
      lp.publishData(bytes, {
        reliable: !!reliable,
        topic: 'lc-control',
        destinationIdentities: toIdentity ? [toIdentity] : undefined,
      });
    } catch (e) { /* channel not ready or not permitted */ }
  }
  function onControlData(cb) { controlDataCb = cb; }

  window.LetsChatHuddleSfu = {
    start: start,
    stop: stop,
    active: active,
    toggleMute: toggleMute,
    toggleCamera: toggleCamera,
    toggleScreen: toggleScreen,
    sendControl: sendControl,
    onControlData: onControlData,
  };
})();
