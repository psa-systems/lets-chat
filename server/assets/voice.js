// Enclave voice channels: multi-party WebRTC over a full mesh.
//
// Persistent-shell script: loaded once per page load from base.html. On a
// voice channel page ([data-lc-voice-root]) it lets the user join a channel
// and opens one RTCPeerConnection to every other participant.
//
// Signaling rides the chat WebSocket; the server is a dumb per-peer relay.
// Negotiation has no glare at setup: the newest joiner is the sole offerer
// to everyone already there (it receives a "roster"); existing members get
// a "joined" event and build their connection lazily from the joiner's
// offer. The "perfect negotiation" pattern still governs mid-call
// renegotiation (camera toggles) per peer.
//
// Mesh practically caps a channel at ~4-6 participants since there's no
// media server. A joined call is bound to the voice page; navigating away
// (or the reconnect soft-refresh) leaves the channel.
(function () {
  'use strict';

  var root = null;        // current [data-lc-voice-root] element, or null
  var cfg = null;         // { roomId, selfId, selfName, iceServers }
  var joined = false;
  var localStream = null;
  var peers = {};         // user_id -> { pc, polite, makingOffer, name, pending: [] }

  // Screen-share state. While sharing, screenTrack is the live display
  // capture; it sits in localStream in place of the camera and feeds every
  // peer's video sender. restoreCameraAfterShare remembers that the camera
  // was on so the camera track is re-acquired when sharing ends.
  var screenTrack = null;
  var restoreCameraAfterShare = false;
  // Live snapshot of who is in the voice channel right now, keyed by user_id.
  // Kept in sync from the initial server-rendered preview plus VoiceJoined /
  // VoiceLeft / VoiceRoster events. The preview HTML is re-rendered from
  // this map so it never lies about who is on the line.
  var participants = {};  // user_id -> label

  // Keep a handle on the chat socket wrapper regardless of page type.
  document.body.addEventListener('htmx:wsOpen', function (e) {
    if (e.detail && e.detail.socketWrapper) window.__lcWS = e.detail.socketWrapper;
  });

  function wsSend(obj) {
    var ws = window.__lcWS;
    if (!ws) return;
    try { ws.send(JSON.stringify(obj)); } catch (e) { /* socket reconnecting */ }
  }
  function voiceSend(kind, targetUserId, payload) {
    if (!cfg) return;
    wsSend({
      type: 'voice_signal',
      room_id: cfg.roomId,
      target_user_id: targetUserId,
      kind: kind,
      payload: payload || null,
    });
  }

  // ---- dom -----------------------------------------------------------
  function q(sel) { return root ? root.querySelector(sel) : null; }
  function grid() { return q('[data-lc-voice-grid]'); }

  // LC-402: build a participant tile. Theme-token styling lives in main.css
  // (.lc-voice-tile and friends); JS only sets the data-lc-* hooks + content.
  function createTile(userId, label, isSelf) {
    var g = grid();
    if (!g) return;
    if (g.querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]')) return;
    var tile = document.createElement('div');
    tile.setAttribute('data-lc-voice-tile', userId);
    tile.className = 'lc-voice-tile';
    var video = document.createElement('video');
    video.setAttribute('data-lc-voice-video', '');
    video.autoplay = true;
    video.playsInline = true;
    if (isSelf) video.muted = true;
    video.style.display = 'none';
    // Shown whenever this participant has no live camera track - including
    // for users on the default avatar, since /avatars/{id} always resolves.
    var avatar = document.createElement('img');
    avatar.setAttribute('data-lc-voice-avatar', '');
    avatar.className = 'lc-voice-avatar';
    avatar.src = '/avatars/' + encodeURIComponent(userId);
    avatar.alt = label;
    // Mute indicator, revealed via [data-muted] on the tile.
    var mute = document.createElement('span');
    mute.className = 'lc-voice-mute-badge';
    mute.setAttribute('aria-hidden', 'true');
    mute.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor"><path d="M7 4a3 3 0 016 0v4a3 3 0 01-.13.87L7.13 4.13A3 3 0 017 4z"/><path d="M5.5 9.5a.75.75 0 00-1.5 0 6 6 0 002.62 4.96l1.1-1.1A4.5 4.5 0 015.5 9.5z"/><path fill-rule="evenodd" d="M3.28 2.22a.75.75 0 10-1.06 1.06l14.5 14.5a.75.75 0 101.06-1.06l-2.3-2.3A5.98 5.98 0 0016 9.5a.75.75 0 00-1.5 0 4.5 4.5 0 01-.84 2.62l-1.1-1.1c.28-.46.44-1 .44-1.52V9.4l-1.5-1.5v.1l-2.9-2.9L3.28 2.22zM10 16.5a6 6 0 003-.8l-1.13-1.13a4.5 4.5 0 01-4.62-1.08l-1.06 1.06A6 6 0 0010 16.5z" clip-rule="evenodd"/></svg>';
    var name = document.createElement('span');
    name.className = 'lc-voice-name';
    var nameText = document.createElement('span');
    nameText.className = 'lc-voice-name-text';
    nameText.textContent = label;
    name.appendChild(nameText);
    if (isSelf) {
      var you = document.createElement('span');
      you.className = 'lc-voice-you';
      you.textContent = window.__lcS('voiceYou', 'You');
      name.appendChild(you);
    }
    tile.appendChild(video);
    tile.appendChild(avatar);
    tile.appendChild(mute);
    tile.appendChild(name);
    g.appendChild(tile);
    updateWaiting();
  }

  // LC-402: "Waiting for others" placeholder while joined but alone.
  function updateWaiting() {
    var g = grid();
    if (!g) return;
    var tiles = g.querySelectorAll('[data-lc-voice-tile]').length;
    var w = g.querySelector('[data-lc-voice-waiting]');
    if (joined && tiles <= 1) {
      if (!w) {
        w = document.createElement('div');
        w.setAttribute('data-lc-voice-waiting', '');
        w.className = 'lc-voice-waiting';
        w.textContent = window.__lcS('voiceWaiting', 'Waiting for others to join...');
        g.appendChild(w);
      }
    } else if (w) {
      w.remove();
    }
  }

  // LC-402: participant-count chip in the header.
  function updateCount() {
    var chip = q('[data-lc-voice-count]');
    if (!chip) return;
    var n = Object.keys(participants).length;
    var nEl = chip.querySelector('[data-lc-voice-count-n]');
    if (nEl) nEl.textContent = String(n);
    chip.classList.toggle('hidden', !(joined && n > 0));
  }

  // Show the camera feed when this participant has a live video track,
  // otherwise fall back to their avatar.
  function updateTileMedia(userId) {
    var g = grid();
    var t = g && g.querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    if (!t) return;
    var video = t.querySelector('[data-lc-voice-video]');
    var avatar = t.querySelector('[data-lc-voice-avatar]');
    if (!video || !avatar) return;
    var hasVideo;
    if (cfg && userId === cfg.selfId) {
      // Our own track: presence is authoritative (we fully remove it on
      // camera-off). Skip the `muted` check - it is briefly true on a
      // freshly-acquired local track, which would wrongly hide our video.
      hasVideo = !!(localStream && localStream.getVideoTracks().some(function (tr) {
        return tr.readyState === 'live';
      }));
    } else {
      var stream = video.srcObject;
      hasVideo = !!(stream && stream.getVideoTracks().some(function (tr) {
        return tr.readyState === 'live' && !tr.muted;
      }));
    }
    video.style.display = hasVideo ? '' : 'none';
    avatar.style.display = hasVideo ? 'none' : '';
  }
  function tileVideo(userId) {
    var t = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    return t ? t.querySelector('[data-lc-voice-video]') : null;
  }
  function removeTile(userId) {
    var t = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    if (t) t.remove();
    updateWaiting();
  }
  function cssEscape(s) { return String(s).replace(/["\\]/g, '\\$&'); }

  function showJoinedUi(on) {
    var preview = q('[data-lc-voice-preview]');
    var g = grid();
    if (preview) preview.style.display = on ? 'none' : '';
    if (g) g.style.display = on ? 'grid' : 'none';
    toggle(q('[data-lc-voice-join]'), !on);
    toggle(q('[data-lc-voice-mute]'), on);
    toggle(q('[data-lc-voice-camera]'), on);
    toggle(q('[data-lc-voice-screen]'), on);
    toggle(q('[data-lc-voice-leave]'), on);
    toggle(q('[data-lc-transcribe-toggle]'), on); // LC-393 Phase 2
    toggle(q('[data-lc-voice-sep]'), on);              // LC-402 control grouping
    toggle(q('[data-lc-transcript-panel-toggle]'), on); // LC-402 transcript drawer
    updateCount();
    updateWaiting();
  }
  function toggle(el, show) { if (el) el.classList[show ? 'remove' : 'add']('hidden'); }

  // Seed `participants` from the server-rendered preview so we know who is
  // already in the channel even before any WS event arrives.
  function seedParticipantsFromDom() {
    participants = {};
    if (!root) return;
    var names = root.querySelectorAll('[data-lc-voice-preview-name]');
    Array.prototype.forEach.call(names, function (el) {
      var uid = el.getAttribute('data-lc-voice-preview-name');
      if (uid) participants[uid] = el.textContent || uid;
    });
  }

  // Re-render the participants preview from the `participants` map. Called
  // on every join/leave event and just before showing the preview after the
  // local user leaves, so the line never shows stale data.
  function renderPreview() {
    var preview = q('[data-lc-voice-preview]');
    if (!preview) return;
    var empty = preview.querySelector('[data-lc-voice-preview-empty]');
    var list = preview.querySelector('[data-lc-voice-preview-list]');
    var names = preview.querySelector('[data-lc-voice-preview-names]');
    if (!empty || !list || !names) return;
    var ids = Object.keys(participants);
    updateCount(); // LC-402
    if (ids.length === 0) {
      empty.classList.remove('hidden');
      list.classList.add('hidden');
      names.replaceChildren();
      return;
    }
    empty.classList.add('hidden');
    list.classList.remove('hidden');
    names.replaceChildren();
    ids.forEach(function (uid, idx) {
      var span = document.createElement('span');
      span.className = 'font-medium text-content';
      span.setAttribute('data-lc-voice-preview-name', uid);
      span.textContent = participants[uid] || uid;
      names.appendChild(span);
      if (idx < ids.length - 1) names.appendChild(document.createTextNode(', '));
    });
  }

  // ---- active-speaker detection (LC-402) -----------------------------
  // A single polling loop taps each tile's audio through a Web Audio analyser
  // and toggles data-speaking on the tile (CSS draws the highlight ring). Fully
  // self-contained and feature-detected so it can never break the call.
  var audioCtx = null;
  var analysers = {};   // MediaStream.id -> { an, data, src }
  var speakLoop = 0;
  function ensureAudioCtx() {
    if (audioCtx) return audioCtx;
    var AC = window.AudioContext || window.webkitAudioContext;
    if (!AC) return null;
    try { audioCtx = new AC(); } catch (e) { audioCtx = null; }
    return audioCtx;
  }
  function analyserFor(stream) {
    if (!stream || !stream.getAudioTracks || !stream.getAudioTracks().length) return null;
    if (analysers[stream.id]) return analysers[stream.id];
    var ctx = ensureAudioCtx();
    if (!ctx) return null;
    try {
      var src = ctx.createMediaStreamSource(stream);
      var an = ctx.createAnalyser();
      an.fftSize = 512;
      src.connect(an); // tap only - not connected to destination, so no echo
      analysers[stream.id] = { an: an, data: new Uint8Array(an.frequencyBinCount), src: src };
      return analysers[stream.id];
    } catch (e) { return null; }
  }
  function levelOf(stream) {
    var a = analyserFor(stream);
    if (!a) return 0;
    a.an.getByteFrequencyData(a.data);
    var sum = 0;
    for (var i = 0; i < a.data.length; i++) sum += a.data[i];
    return sum / a.data.length; // 0..255
  }
  function pollSpeaking() {
    var g = grid();
    if (!g) return;
    var tiles = g.querySelectorAll('[data-lc-voice-tile]');
    for (var i = 0; i < tiles.length; i++) {
      var tile = tiles[i];
      var isSelf = cfg && tile.getAttribute('data-lc-voice-tile') === cfg.selfId;
      var v = tile.querySelector('[data-lc-voice-video]');
      var stream = isSelf ? localStream : (v && v.srcObject);
      var speaking = false;
      if (stream && tile.getAttribute('data-muted') !== 'true') speaking = levelOf(stream) > 18;
      tile.setAttribute('data-speaking', speaking ? 'true' : 'false');
    }
  }
  function startSpeaking() {
    var ctx = ensureAudioCtx();
    if (ctx && ctx.state === 'suspended') { try { ctx.resume(); } catch (e) {} }
    if (speakLoop) return;
    speakLoop = setInterval(pollSpeaking, 250);
  }
  function stopSpeaking() {
    if (speakLoop) { clearInterval(speakLoop); speakLoop = 0; }
    Object.keys(analysers).forEach(function (id) {
      try { analysers[id].src.disconnect(); } catch (e) {}
    });
    analysers = {};
    var g = grid();
    if (g) Array.prototype.forEach.call(g.querySelectorAll('[data-lc-voice-tile]'), function (t) {
      t.removeAttribute('data-speaking');
    });
  }

  // ---- media ---------------------------------------------------------
  function getMedia(withVideo) {
    // LC-144: honor the user's pinned mic/camera via the shared module.
    if (window.LetsChatDevices) return window.LetsChatDevices.getUserMedia(withVideo);
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      return Promise.reject(new Error('getUserMedia unavailable'));
    }
    return navigator.mediaDevices.getUserMedia(
      withVideo ? { audio: true, video: true } : { audio: true }
    );
  }
  // LC-144: camera-only acquisition honoring the pinned camera.
  function getCamera() {
    if (window.LetsChatDevices) return window.LetsChatDevices.getCamera();
    return navigator.mediaDevices.getUserMedia({ video: true });
  }
  function hasLocalVideo() {
    return !!(localStream && localStream.getVideoTracks().length > 0);
  }
  function isSharingScreen() {
    return !!screenTrack;
  }
  function hasLocalCamera() {
    return hasLocalVideo() && !isSharingScreen();
  }

  // ---- per-peer connection ------------------------------------------
  function iceConfig() {
    try { return { iceServers: JSON.parse(cfg.iceServers) }; }
    catch (e) { return { iceServers: [{ urls: 'stun:stun.l.google.com:19302' }] }; }
  }

  function makePc(userId) {
    var p = peers[userId];
    var pc = new RTCPeerConnection(iceConfig());
    p.pc = pc;
    pc.onicecandidate = function (ev) {
      if (ev.candidate) voiceSend('ice', userId, JSON.stringify(ev.candidate));
    };
    pc.ontrack = function (ev) {
      var v = tileVideo(userId);
      if (v && ev.streams && ev.streams[0]) {
        v.srcObject = ev.streams[0];
        // LC-144: route this peer's audio to the user's pinned speaker.
        if (window.LetsChatDevices) window.LetsChatDevices.applySpeaker(v);
        ev.streams[0].onaddtrack = function () { updateTileMedia(userId); };
        ev.streams[0].onremovetrack = function () { updateTileMedia(userId); };
      }
      if (ev.track) {
        ev.track.onmute = function () { updateTileMedia(userId); };
        ev.track.onunmute = function () { updateTileMedia(userId); };
        ev.track.onended = function () { updateTileMedia(userId); };
      }
      updateTileMedia(userId);
    };
    pc.onnegotiationneeded = function () {
      p.makingOffer = true;
      pc.setLocalDescription()
        .then(function () { voiceSend('offer', userId, JSON.stringify(pc.localDescription)); })
        .catch(function (e) { console.warn('voice: negotiation failed', e); })
        .finally(function () { p.makingOffer = false; });
    };
    pc.onconnectionstatechange = function () {
      // Do NOT remove the peer here. In a mesh, one participant leaving
      // closes their connections, which transiently flaps *other* peers'
      // connections through "failed" - auto-removing on that state would
      // cascade and drop everyone. Peers are removed only by the explicit
      // VoiceLeft event, which the server sends reliably on both a clean
      // leave and a disconnect. (The 1:1 DM call in call.js does tear down
      // on connection failure - that is correct there, but not for a mesh.)
      if (p.pc === pc && pc.connectionState === 'failed') {
        console.warn('voice: connection to', userId, 'failed; awaiting VoiceLeft');
      }
    };
    return pc;
  }

  function addLocalTracks(pc) {
    if (!pc || !localStream) return;
    var sharing = isSharingScreen();
    localStream.getTracks().forEach(function (t) {
      // Only one outgoing video track per peer. While sharing, the screen
      // track wins so a new joiner mid-share also receives the share rather
      // than the (transiently still present) camera.
      if (sharing && t.kind === 'video' && t !== screenTrack) return;
      pc.addTrack(t, localStream);
    });
  }

  function flushCandidates(userId) {
    var p = peers[userId];
    if (!p || !p.pc) return;
    var list = p.pending;
    p.pending = [];
    list.forEach(function (c) {
      p.pc.addIceCandidate(c).catch(function (e) {
        console.warn('voice: addIceCandidate failed', e);
      });
    });
  }

  // Register a peer. `iAmOfferer` => this side initiates (newest joiner
  // offering to an existing member); otherwise the pc is built lazily from
  // the peer's offer.
  function addPeer(userId, name, iAmOfferer) {
    if (peers[userId]) return;
    peers[userId] = { pc: null, polite: !iAmOfferer, makingOffer: false, name: name, pending: [] };
    createTile(userId, name, false);
    if (iAmOfferer) {
      makePc(userId);
      addLocalTracks(peers[userId].pc); // -> negotiationneeded -> offer
    }
  }

  function removePeer(userId) {
    var p = peers[userId];
    if (!p) return;
    if (p.pc) {
      p.pc.onicecandidate = p.pc.ontrack = p.pc.onnegotiationneeded = p.pc.onconnectionstatechange = null;
      try { p.pc.close(); } catch (e) {}
    }
    delete peers[userId];
    removeTile(userId);
  }

  function onOffer(userId, name, sdp) {
    var p = peers[userId];
    if (!p) { addPeer(userId, name, false); p = peers[userId]; }
    var desc;
    try { desc = JSON.parse(sdp); } catch (e) { return; }
    var firstOffer = !p.pc;
    if (firstOffer) makePc(userId);
    var pc = p.pc;
    var collision = p.makingOffer || pc.signalingState !== 'stable';
    p.ignoreOffer = !p.polite && collision;
    if (p.ignoreOffer) return;
    pc.setRemoteDescription(desc)
      .then(function () {
        if (firstOffer) addLocalTracks(pc);
        flushCandidates(userId);
        return pc.setLocalDescription();
      })
      .then(function () { voiceSend('answer', userId, JSON.stringify(pc.localDescription)); })
      .catch(function (e) { console.warn('voice: onOffer failed', e); });
  }

  function onAnswer(userId, sdp) {
    var p = peers[userId];
    if (!p || !p.pc) return;
    var desc;
    try { desc = JSON.parse(sdp); } catch (e) { return; }
    p.pc.setRemoteDescription(desc)
      .then(function () { flushCandidates(userId); })
      .catch(function (e) { console.warn('voice: onAnswer failed', e); });
  }

  function onIce(userId, payload) {
    var p = peers[userId];
    if (!p || !payload) return;
    var cand;
    try { cand = JSON.parse(payload); } catch (e) { return; }
    if (!p.pc || !p.pc.remoteDescription) { p.pending.push(cand); return; }
    p.pc.addIceCandidate(cand).catch(function (e) {
      if (!p.ignoreOffer) console.warn('voice: addIceCandidate failed', e);
    });
  }

  // ---- join / leave --------------------------------------------------
  function join() {
    if (joined || !cfg) return;
    getMedia(false).then(function (stream) {
      localStream = stream;
      joined = true;
      // Record ourselves so the preview is correct if we leave before any
      // VoiceJoined echoes back; the roster event will overwrite this.
      participants[cfg.selfId] = cfg.selfName;
      showJoinedUi(true);
      createTile(cfg.selfId, cfg.selfName, true);
      startSpeaking();
      var sv = tileVideo(cfg.selfId);
      // Re-assign (null first) so the <video> re-renders with the changed
      // track set - assigning the same stream object can be a no-op.
      if (sv) { sv.srcObject = null; sv.srcObject = localStream; }
      updateTileMedia(cfg.selfId);
      setCameraBtn();
      wsSend({ type: 'voice_join', room_id: cfg.roomId });
      // LC-393 Phase 2: publish the channel room so transcribe.js can open a
      // transcription session, and signal that we are in a live call.
      window.__lcVoiceRoom = cfg.roomId;
      try { document.dispatchEvent(new CustomEvent('lc:voice-joined')); } catch (e) {}
    }).catch(function () {
      alert(window.__lcS('callNoMic', 'Could not access your microphone.'));
    });
  }

  function leave() {
    if (!joined) return;
    stopSpeaking();
    wsSend({ type: 'voice_leave', room_id: cfg.roomId });
    Object.keys(peers).forEach(removePeer);
    stopScreen();
    if (localStream) {
      localStream.getTracks().forEach(function (t) { try { t.stop(); } catch (e) {} });
      localStream = null;
    }
    removeTile(cfg.selfId);
    var g = grid();
    if (g) g.replaceChildren();
    joined = false;
    var ss = q('[data-lc-voice-screen]');
    if (ss) ss.textContent = window.__lcS('callShareScreen', 'Share screen');
    // Drop ourselves from the participants map and re-render the preview
    // before un-hiding it. Otherwise the leaver would see whatever was
    // server-rendered at page load, which is often "no one is here yet" even
    // when the call is still busy.
    if (cfg) delete participants[cfg.selfId];
    renderPreview();
    showJoinedUi(false);
    // LC-393 Phase 2: we left the channel - transcribe.js stops our local
    // capture (the session itself continues for whoever is still here).
    window.__lcVoiceRoom = null;
    try { document.dispatchEvent(new CustomEvent('lc:voice-left')); } catch (e) {}
  }

  function toggleMute() {
    if (!localStream) return;
    var on = true;
    localStream.getAudioTracks().forEach(function (t) { t.enabled = !t.enabled; on = t.enabled; });
    var muted = !on;
    var btn = q('[data-lc-voice-mute]');
    if (btn) {
      btn.textContent = on ? window.__lcS('callMute', 'Mute') : window.__lcS('callUnmute', 'Unmute');
      btn.setAttribute('aria-pressed', muted ? 'true' : 'false'); // LC-402 on/off state
    }
    // LC-402: reflect our own mute on our tile.
    var tile = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(cfg.selfId) + '"]');
    if (tile) tile.setAttribute('data-muted', muted ? 'true' : 'false');
  }

  function setCameraBtn() {
    var btn = q('[data-lc-voice-camera]');
    if (!btn) return;
    btn.textContent = hasLocalCamera() ? window.__lcS('callStopVideo', 'Stop video') : window.__lcS('callStartVideo', 'Start video');
    btn.setAttribute('aria-pressed', hasLocalCamera() ? 'true' : 'false'); // LC-402 on/off state
    // While the screen-share track owns the video sender on every peer,
    // toggling the camera would race for the same outgoing slot.
    if (isSharingScreen()) btn.setAttribute('disabled', '');
    else btn.removeAttribute('disabled');
  }
  function setScreenBtn() {
    var btn = q('[data-lc-voice-screen]');
    if (!btn) return;
    btn.textContent = isSharingScreen() ? 'Stop sharing' : 'Share screen';
    btn.setAttribute('aria-pressed', isSharingScreen() ? 'true' : 'false'); // LC-402 on/off state
  }

  // Camera on/off mid-call: acquire or release the video track and add it to
  // (or remove it from) every peer connection, which renegotiates each.
  function toggleCamera() {
    if (!joined || !localStream) return;
    if (isSharingScreen()) return;  // setCameraBtn disables the control; guard the dispatch path too.
    var existing = localStream.getVideoTracks()[0];
    if (existing) {
      Object.keys(peers).forEach(function (uid) {
        var pc = peers[uid].pc;
        if (!pc) return;
        var sender = pc.getSenders().find(function (s) { return s.track === existing; });
        if (sender) pc.removeTrack(sender);
      });
      existing.stop();
      localStream.removeTrack(existing);
      var sv = tileVideo(cfg.selfId);
      // Re-assign (null first) so the <video> re-renders with the changed
      // track set - assigning the same stream object can be a no-op.
      if (sv) { sv.srcObject = null; sv.srcObject = localStream; }
      updateTileMedia(cfg.selfId);
      setCameraBtn();
    } else {
      getCamera().then(function (vstream) {
        var vtrack = vstream.getVideoTracks()[0];
        if (!vtrack) return;
        if (!joined || !localStream) { try { vtrack.stop(); } catch (e) {} return; }
        localStream.addTrack(vtrack);
        Object.keys(peers).forEach(function (uid) {
          if (peers[uid].pc) peers[uid].pc.addTrack(vtrack, localStream);
        });
        var sv = tileVideo(cfg.selfId);
        // Re-assign (null first) so the <video> re-renders with the changed
        // track set - assigning the same stream object can be a no-op.
        if (sv) { sv.srcObject = null; sv.srcObject = localStream; }
        updateTileMedia(cfg.selfId);
        setCameraBtn();
      }).catch(function () { alert(window.__lcS('callNoCamera', 'Could not access your camera.')); });
    }
  }

  // Find the sender on one peer connection that transports our outgoing
  // video track (camera or screen). Walks transceivers because removeTrack
  // leaves the sender in place with a null track, and we still want to
  // reuse that slot.
  function findVideoSenderOn(pc) {
    if (!pc || !pc.getTransceivers) return null;
    var ts = pc.getTransceivers();
    for (var i = 0; i < ts.length; i++) {
      var t = ts[i];
      if (t.receiver && t.receiver.track && t.receiver.track.kind === 'video') {
        return t.sender;
      }
      if (t.sender && t.sender.track && t.sender.track.kind === 'video') {
        return t.sender;
      }
    }
    return null;
  }

  // Swap one peer's outgoing video track to `track`. Prefer replaceTrack to
  // avoid renegotiation; fall back to removeTrack + addTrack so the peer
  // actually receives the new track when the browser rejects the swap
  // (codec/parameter mismatch is the common case for getDisplayMedia).
  // When `track` is null and no sender exists yet, this is a no-op.
  function swapPeerVideo(pc, track) {
    if (!pc) return Promise.resolve();
    var sender = findVideoSenderOn(pc);
    if (sender && sender.track) {
      return sender.replaceTrack(track).catch(function (e) {
        console.warn('voice: replaceTrack failed, falling back to addTrack', e);
        try { pc.removeTrack(sender); } catch (ee) {}
        if (track) {
          try { pc.addTrack(track, localStream); } catch (ee) {}
        }
      });
    }
    if (sender && !sender.track && track) {
      return sender.replaceTrack(track).catch(function (e) {
        console.warn('voice: replaceTrack on idle sender failed', e);
        try { pc.addTrack(track, localStream); } catch (ee) {}
      });
    }
    if (track) {
      try { pc.addTrack(track, localStream); } catch (e) {
        console.warn('voice: addTrack failed', e);
      }
    }
    return Promise.resolve();
  }

  // Acquire screen capture and route it to every peer.
  function toggleScreen() {
    if (!joined || !localStream) return;
    if (isSharingScreen()) { endScreenShare(); return; }
    if (!navigator.mediaDevices || !navigator.mediaDevices.getDisplayMedia) {
      alert(window.__lcS('callNoScreenshare', 'Screen sharing is not supported by your browser.'));
      return;
    }
    navigator.mediaDevices.getDisplayMedia({ video: true, audio: false }).then(function (dstream) {
      var dtrack = dstream.getVideoTracks()[0];
      if (!dtrack) return;
      if (!joined || !localStream) { try { dtrack.stop(); } catch (e) {} return; }
      // Hint the encoder that this is mostly static content with sharp
      // edges; browsers that honor this bias toward higher resolution /
      // lower frame rate, which is what screen content actually wants.
      try { dtrack.contentHint = 'detail'; } catch (e) {}
      var camera = localStream.getVideoTracks()[0];
      restoreCameraAfterShare = !!camera;
      // Stamp the screen-share state before the async swaps so any peer
      // added during the renegotiation window (addLocalTracks() observes
      // screenTrack and localStream) also picks up the screen track.
      localStream.addTrack(dtrack);
      screenTrack = dtrack;
      // User clicks "Stop sharing" in the browser chrome -> end the share.
      dtrack.onended = function () { endScreenShare(); };
      var sv = tileVideo(cfg.selfId);
      if (sv) { sv.srcObject = null; sv.srcObject = localStream; }
      updateTileMedia(cfg.selfId);
      setCameraBtn();
      setScreenBtn();
      // Swap every existing peer's outgoing video slot to the screen track
      // while the camera is still alive (replaceTrack must not race a stop).
      // After all peers settle, drop the camera.
      var swaps = Object.keys(peers).map(function (uid) {
        return swapPeerVideo(peers[uid].pc, dtrack);
      });
      Promise.all(swaps).then(function () {
        if (camera) {
          try { camera.stop(); } catch (e) {}
          try { localStream.removeTrack(camera); } catch (e) {}
        }
      });
    }).catch(function (e) {
      // User-cancel from the picker lands here too; nothing to undo.
      if (e && e.name !== 'NotAllowedError') console.warn('voice: getDisplayMedia failed', e);
    });
  }

  function stopScreen() {
    if (screenTrack) {
      screenTrack.onended = null;
      try { screenTrack.stop(); } catch (e) {}
      screenTrack = null;
    }
    restoreCameraAfterShare = false;
  }

  // Drop the screen track from every peer. If the camera was on when sharing
  // started, re-acquire it and swap it back in; otherwise leave the senders
  // with null tracks so each peer's tile drops back to the avatar.
  function endScreenShare() {
    if (!isSharingScreen()) return;
    var oldScreen = screenTrack;
    var wantCamera = restoreCameraAfterShare;
    // Remove the screen track from localStream first so the self tile reflects
    // reality immediately even if camera re-acquisition takes a moment.
    try { localStream.removeTrack(oldScreen); } catch (e) {}
    stopScreen();
    var sv = tileVideo(cfg.selfId);
    if (sv) { sv.srcObject = null; sv.srcObject = localStream; }
    updateTileMedia(cfg.selfId);

    function swapAllTo(track) {
      var swaps = Object.keys(peers).map(function (uid) {
        return swapPeerVideo(peers[uid].pc, track);
      });
      return Promise.all(swaps);
    }

    if (wantCamera) {
      getCamera().then(function (vstream) {
        var vtrack = vstream.getVideoTracks()[0];
        if (!vtrack) return swapAllTo(null);
        if (!joined || !localStream) { try { vtrack.stop(); } catch (e) {} return; }
        localStream.addTrack(vtrack);
        return swapAllTo(vtrack).then(function () {
          var sv2 = tileVideo(cfg.selfId);
          if (sv2) { sv2.srcObject = null; sv2.srcObject = localStream; }
          updateTileMedia(cfg.selfId);
        });
      }).catch(function () {
        // Camera unavailable on the way back: leave senders empty.
        swapAllTo(null);
      }).finally(function () {
        setCameraBtn();
        setScreenBtn();
      });
    } else {
      swapAllTo(null).finally(function () {
        setCameraBtn();
        setScreenBtn();
      });
    }
  }

  // ---- inbound dispatch ---------------------------------------------
  function handleEvent(node) {
    if (!cfg) return;
    var roomId = node.getAttribute('data-room-id');
    if (String(cfg.roomId) !== String(roomId)) return; // not this channel
    var kind = node.getAttribute('data-kind');
    var userId = node.getAttribute('data-user-id');
    var username = node.getAttribute('data-username') || 'Someone';
    var payload = node.getAttribute('data-payload');

    if (kind === 'roster') {
      if (!joined) return;
      var list;
      try { list = JSON.parse(node.getAttribute('data-peers') || '[]'); } catch (e) { list = []; }
      // Rebuild the participants map from the authoritative server roster
      // plus ourselves; the preview will reflect this once we leave.
      participants = {};
      participants[cfg.selfId] = cfg.selfName;
      list.forEach(function (pair) {
        participants[pair[0]] = pair[1];
        // We are the newest joiner: offer to everyone already here.
        addPeer(pair[0], pair[1], true);
      });
      renderPreview();
      return;
    }
    if (kind === 'joined') {
      // Track the new participant for the preview regardless of whether we
      // are joined to the call ourselves.
      if (userId) participants[userId] = username;
      renderPreview();
      // Someone joined after us; they will offer, we answer. Skip when the
      // event is about us (we already created our own tile in join()).
      if (joined && userId !== cfg.selfId) addPeer(userId, username, false);
      return;
    }
    if (kind === 'left') {
      if (userId) delete participants[userId];
      renderPreview();
      removePeer(userId);
      return;
    }
    if (!joined) return;
    if (kind === 'offer') onOffer(userId, username, payload);
    else if (kind === 'answer') onAnswer(userId, payload);
    else if (kind === 'ice') onIce(userId, payload);
  }

  function watchBus() {
    var bus = document.getElementById('lc-voice-bus');
    if (!bus) return;
    new MutationObserver(function (muts) {
      muts.forEach(function (m) {
        Array.prototype.forEach.call(m.addedNodes, function (n) {
          if (n.nodeType !== 1) return;
          if (n.hasAttribute('data-lc-voice-event')) handleEvent(n);
          else if (n.querySelector) {
            var c = n.querySelector('[data-lc-voice-event]');
            if (c) handleEvent(c);
          }
        });
      });
      bus.replaceChildren();
    }).observe(bus, { childList: true });
  }

  // ---- wiring --------------------------------------------------------
  function bindRoot(el) {
    if (el === root) return;
    if (joined) leave(); // navigated to a different voice page
    root = el;
    cfg = {
      roomId: parseInt(el.getAttribute('data-room-id'), 10),
      selfId: el.getAttribute('data-self-id'),
      selfName: el.getAttribute('data-self-name') || 'You',
      iceServers: el.getAttribute('data-ice-servers') || '[]',
    };
    joined = false;
    seedParticipantsFromDom();
  }

  function scan() {
    var el = document.querySelector('[data-lc-voice-root]');
    if (el) {
      bindRoot(el);
    } else if (root) {
      if (joined) leave();
      root = null;
      cfg = null;
    }
  }

  document.body.addEventListener('click', function (e) {
    var t = e.target;
    if (!t || !t.closest) return;
    if (t.closest('[data-lc-voice-join]')) { join(); return; }
    if (t.closest('[data-lc-voice-leave]')) { leave(); return; }
    if (t.closest('[data-lc-voice-mute]')) { toggleMute(); return; }
    if (t.closest('[data-lc-voice-camera]')) { toggleCamera(); return; }
    if (t.closest('[data-lc-voice-screen]')) { toggleScreen(); return; }
  });

  function onReady(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }
  onReady(function () {
    watchBus();
    scan();
    document.body.addEventListener('htmx:afterSettle', scan);
  });

  // LC-144: re-route every peer tile's audio when the speaker is changed
  // mid-call from the device picker.
  document.addEventListener('lc:speaker-change', function () {
    if (!window.LetsChatDevices) return;
    Object.keys(peers).forEach(function (uid) {
      window.LetsChatDevices.applySpeaker(tileVideo(uid));
    });
  });

  // Exposed so the 1:1 DM call (call.js) can drop us out of an enclave
  // voice channel when accepting an incoming DM call - otherwise the
  // browser would hold two simultaneous mics/cameras open.
  window.LetsChatVoice = {
    leave: function () { if (joined) leave(); },
    isJoined: function () { return joined; },
  };
})();
