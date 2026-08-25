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
  var cfg = null;         // { roomId, selfId, selfName, iceServers, sfu }
  var joined = false;
  // LC-610: when the room's huddle runs over the SFU, media is owned by
  // huddle_sfu.js and this stays true for the session. The mesh branch below is
  // untouched; every mesh-only step is guarded on `!sfu`.
  var sfu = false;
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
  // LC-784: user_id -> avatar version token. Seeded from the server-rendered
  // preview chips and each VoiceJoined / VoiceRoster event so avatarUrl() can
  // build an immutable `/avatars/{id}?v={token}` instead of the bare, per-
  // navigation-revalidating URL. A missing entry falls back to the bare URL,
  // which still resolves (no-cache) - so an unversioned path never breaks.
  var avatarVersions = {};
  var selfMuted = false;  // LC-402: our own mic state, broadcast to the channel
  var selfSharing = false; // LC-408: our own screen-share state, broadcast too
  var sfuCamOn = false;   // LC-610: our SFU camera state (localStream is null there)

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

  // LC-764: opt-in call diagnostics for the mesh path (the SFU path logs from
  // huddle_sfu.js). Set `window.__lcCallDebug = true` in the console before
  // reproducing to log each local audio track's real state on both sides of a
  // mute toggle. Off by default, so a normal call logs nothing.
  function dbg() {
    if (!window.__lcCallDebug || !window.console || !console.log) return;
    try { console.log.apply(console, ['[lc-call mesh]'].concat([].slice.call(arguments))); } catch (e) {}
  }
  function audioTrackStates() {
    if (!localStream) return 'no local stream';
    return localStream.getAudioTracks().map(function (t) {
      return { enabled: t.enabled, muted: t.muted, readyState: t.readyState };
    });
  }

  // ---- dom -----------------------------------------------------------
  function q(sel) { return root ? root.querySelector(sel) : null; }
  function grid() { return q('[data-lc-voice-grid]'); }

  // LC-416: control buttons are now icon + .lc-cbtn-label; set the label span so
  // the leading icon survives (fall back to the button for any plain-text one).
  function setLabel(btn, text) {
    if (!btn) return;
    var l = btn.querySelector('.lc-cbtn-label');
    if (l) l.textContent = text; else btn.textContent = text;
  }

  // LC-402: build a participant tile. Theme-token styling lives in main.css
  // (.lc-voice-tile and friends); JS only sets the data-lc-* hooks + content.
  // LC-784: build a versioned avatar URL from the id + the tracked version
  // token. With a token the route answers `immutable` and the browser skips
  // revalidation; without one (an SFU tile whose identity came from LiveKit, or
  // an older frame) it falls back to the bare URL, which still resolves.
  function avatarUrl(uid) {
    var v = avatarVersions[uid];
    return '/avatars/' + encodeURIComponent(uid) + (v ? '?v=' + encodeURIComponent(v) : '');
  }

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
    avatar.src = avatarUrl(userId);
    avatar.alt = label;
    // Mute indicator, revealed via [data-muted] on the tile.
    var mute = document.createElement('span');
    mute.className = 'lc-voice-mute-badge';
    mute.setAttribute('aria-hidden', 'true');
    mute.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor"><path d="M7 4a3 3 0 016 0v4a3 3 0 01-.13.87L7.13 4.13A3 3 0 017 4z"/><path d="M5.5 9.5a.75.75 0 00-1.5 0 6 6 0 002.62 4.96l1.1-1.1A4.5 4.5 0 015.5 9.5z"/><path fill-rule="evenodd" d="M3.28 2.22a.75.75 0 10-1.06 1.06l14.5 14.5a.75.75 0 101.06-1.06l-2.3-2.3A5.98 5.98 0 0016 9.5a.75.75 0 00-1.5 0 4.5 4.5 0 01-.84 2.62l-1.1-1.1c.28-.46.44-1 .44-1.52V9.4l-1.5-1.5v.1l-2.9-2.9L3.28 2.22zM10 16.5a6 6 0 003-.8l-1.13-1.13a4.5 4.5 0 01-4.62-1.08l-1.06 1.06A6 6 0 0010 16.5z" clip-rule="evenodd"/></svg>';
    // LC-408: "presenting" badge, revealed via [data-screen] on the tile.
    var screen = document.createElement('span');
    screen.className = 'lc-voice-screen-badge';
    screen.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true"><path d="M3 4a2 2 0 00-2 2v6a2 2 0 002 2h5v2H6a1 1 0 100 2h8a1 1 0 100-2h-2v-2h5a2 2 0 002-2V6a2 2 0 00-2-2H3z"/></svg>';
    var screenLabel = document.createElement('span');
    screenLabel.textContent = window.__lcS('voicePresenting', 'Presenting');
    screen.appendChild(screenLabel);
    var name = document.createElement('span');
    name.className = 'lc-voice-name';
    // LC-410: per-peer connection-quality glyph. Hidden until the tile gets a
    // data-quality (set by the quality monitor); never set on the self tile,
    // which has no peer connection. Signal bars, recoloured by CSS per state.
    var quality = document.createElement('span');
    quality.className = 'lc-voice-quality';
    quality.setAttribute('data-lc-voice-quality', '');
    quality.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true"><rect x="2" y="12" width="3" height="6" rx="1"/><rect x="8.5" y="8" width="3" height="10" rx="1"/><rect x="15" y="3" width="3" height="15" rx="1"/></svg>';
    name.appendChild(quality);
    // LC-416: animated active-speaker equalizer (CSS shows it on data-speaking).
    var speaking = document.createElement('span');
    speaking.className = 'lc-voice-speaking';
    speaking.setAttribute('aria-hidden', 'true');
    speaking.innerHTML = '<i></i><i></i><i></i>';
    name.appendChild(speaking);
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
    tile.appendChild(screen);
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
    // LC-700: expose the tile's media state so the huddle dock can lay out
    // audio-only participants as compact chips (pure CSS :has() on the root)
    // and only grow to full tiles when a camera or screen-share is live.
    t.setAttribute('data-media', hasVideo ? 'video' : 'audio');
    // NOTE: do NOT clear data-screen here based on `hasVideo`. The screen-share
    // pin is owned solely by the explicit voice_screen signal (applyScreen). On
    // the RECEIVER the voice_screen=true signal lands before the screen video
    // track is live, and this fn runs from ontrack/onmute during that window -
    // clearing the pin here wiped it and the stage never engaged for peers
    // (LC-408 receiver-pin bug). Stop is handled by voice_screen=false; a
    // crashed sharer is handled by VoiceLeft removing the tile.
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

  // LC-493: a huddle root ([data-lc-huddle]) stays hidden until the viewer is
  // on the line OR someone else is (so an idle group room shows nothing but the
  // header "Huddle" button). Voice-channel roots have no [data-lc-huddle] and
  // are always visible. Cheap + idempotent; called wherever join state or the
  // participant set changes.
  function applyHuddleVisibility() {
    if (!root || !root.hasAttribute('data-lc-huddle')) return;
    var active = joined || Object.keys(participants).length > 0;
    root.classList.toggle('hidden', !active);
  }

  function showJoinedUi(on) {
    var preview = q('[data-lc-voice-preview]');
    var g = grid();
    if (preview) preview.style.display = on ? 'none' : '';
    // LC-408: class toggle, not inline display, so the screen-share stage CSS
    // (which switches the grid to flex) is not overridden by an inline style.
    if (g) g.classList.toggle('lc-voice-grid--hidden', !on);
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
    applyHuddleVisibility(); // LC-493
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
      // LC-416: chips carry the label in data-lc-label (was the text node).
      if (uid) participants[uid] = el.getAttribute('data-lc-label') || (el.textContent || '').trim() || uid;
      // LC-784: chips also carry the avatar version so a re-rendered preview
      // keeps immutable avatar URLs before any WS event arrives.
      var ver = el.getAttribute('data-lc-avatar-version');
      if (uid && ver) avatarVersions[uid] = ver;
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
    var heading = preview.querySelector('[data-lc-voice-preview-heading]');
    if (!empty || !list || !names) return;
    var ids = Object.keys(participants);
    updateCount(); // LC-402
    if (ids.length === 0) {
      empty.classList.remove('hidden');
      list.classList.add('hidden');
      names.replaceChildren();
      // LC-789: re-evaluate huddle-bar visibility on the empty path too. For a
      // member only VIEWING the room (never joined), the bar is shown purely by
      // applyHuddleVisibility() (visible while someone is on the line). When the
      // last participant left, this branch returned before that call, so the bar
      // hung open in the viewer's room after the huddle had emptied.
      applyHuddleVisibility(); // LC-493
      return;
    }
    empty.classList.add('hidden');
    list.classList.remove('hidden');
    // LC-416: heading templates come from the server-rendered data attrs, so
    // there is no separate JS i18n catalog to keep in step.
    if (heading) {
      if (ids.length === 1) {
        heading.textContent = preview.getAttribute('data-lc-lobby-one') || '';
      } else {
        heading.textContent = ids.length + ' ' + (preview.getAttribute('data-lc-lobby-other') || '');
      }
    }
    // LC-416: rebuild the avatar chips (same structure the server renders).
    names.replaceChildren();
    ids.forEach(function (uid) {
      var label = participants[uid] || uid;
      var chip = document.createElement('span');
      chip.className = 'lc-voice-lobby-chip';
      chip.setAttribute('data-lc-voice-preview-name', uid);
      chip.setAttribute('data-lc-label', label);
      var img = document.createElement('img');
      img.className = 'lc-voice-lobby-avatar';
      img.src = avatarUrl(uid);
      img.alt = label;
      var nm = document.createElement('span');
      nm.className = 'lc-voice-lobby-name';
      nm.textContent = label;
      chip.appendChild(img);
      chip.appendChild(nm);
      names.appendChild(chip);
    });
    applyHuddleVisibility(); // LC-493
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

  // ---- connection-quality monitor (LC-410) ---------------------------
  // Per-remote-peer health from RTCPeerConnection.connectionState refined by a
  // periodic getStats (packet loss + RTT). The tile carries data-quality
  // (good | degraded | reconnecting | failed); CSS draws the coloured signal
  // glyph. connectionstatechange updates immediately; the loop polls stats.
  var qualityLoop = 0;
  var QUALITY_LABEL = {
    good: 'connGood', degraded: 'connDegraded',
    reconnecting: 'connReconnecting', failed: 'connFailed',
  };
  function setQuality(userId, qval) {
    var t = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    if (!t || t.getAttribute('data-quality') === qval) return;
    t.setAttribute('data-quality', qval);
    var el = t.querySelector('[data-lc-voice-quality]');
    if (el) {
      var label = window.__lcS(QUALITY_LABEL[qval] || 'connGood', qval);
      el.setAttribute('title', label);
      el.setAttribute('aria-label', label);
    }
  }
  // Map a non-connected RTCPeerConnection state to a quality bucket.
  function stateQuality(state) {
    if (state === 'failed') return 'failed';
    if (state === 'disconnected' || state === 'connecting' || state === 'checking') return 'reconnecting';
    return null; // 'new' / 'closed' -> leave as-is
  }
  function applyConnectionState(userId, state) {
    if (state === 'connected') { setQuality(userId, 'good'); return; } // refined by stats
    var q = stateQuality(state);
    if (q) setQuality(userId, q);
  }
  function pollQuality() {
    Object.keys(peers).forEach(function (uid) {
      var p = peers[uid];
      if (!p || !p.pc) return;
      var state = p.pc.connectionState;
      if (state !== 'connected') { applyConnectionState(uid, state); return; }
      p.pc.getStats().then(function (report) {
        var lost = 0, recv = 0, rtt = null;
        report.forEach(function (s) {
          if (s.type === 'inbound-rtp') {
            if (typeof s.packetsLost === 'number') lost += s.packetsLost;
            if (typeof s.packetsReceived === 'number') recv += s.packetsReceived;
          } else if (s.type === 'candidate-pair' && s.nominated &&
                     typeof s.currentRoundTripTime === 'number') {
            rtt = s.currentRoundTripTime;
          }
        });
        var prev = p.qstats || { lost: 0, recv: 0 };
        var dLost = Math.max(0, lost - prev.lost);
        var dRecv = Math.max(0, recv - prev.recv);
        p.qstats = { lost: lost, recv: recv };
        var lossRatio = (dLost + dRecv) > 0 ? dLost / (dLost + dRecv) : 0;
        var degraded = lossRatio > 0.03 || (rtt != null && rtt > 0.35);
        setQuality(uid, degraded ? 'degraded' : 'good');
      }).catch(function (e) {
        // getStats failed on this poll: the quality dot freezes at its last
        // value, so log rather than let it look like a steady connection.
        try { console.warn('[lc-call mesh] quality poll getStats failed', e); } catch (e2) {}
      });
    });
  }
  function startQuality() {
    if (qualityLoop) return;
    qualityLoop = setInterval(pollQuality, 3000);
  }
  function stopQuality() {
    if (qualityLoop) { clearInterval(qualityLoop); qualityLoop = 0; }
  }

  // ---- media ---------------------------------------------------------
  function getMedia(withVideo) {
    // LC-144: honor the user's pinned mic/camera via the shared module.
    if (window.LetsChatDevices) return window.LetsChatDevices.getUserMedia(withVideo);
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      return Promise.reject(new Error('getUserMedia unavailable'));
    }
    // LC-628: keep the echo cancellation / noise suppression / auto gain trio
    // on this degraded path (LetsChatDevices absent) via the shared constraint.
    return navigator.mediaDevices.getUserMedia(
      withVideo
        ? { audio: window.LetsChatMedia.audio(), video: true }
        : { audio: window.LetsChatMedia.audio() }
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
      if (p.pc !== pc) return;
      applyConnectionState(userId, pc.connectionState); // LC-410: live quality
      if (pc.connectionState === 'failed') {
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
    if (cfg.sfu) { joinSfu(); return; }
    getMedia(false).then(function (stream) {
      localStream = stream;
      joined = true;
      selfMuted = false; // LC-402: fresh session starts unmuted
      selfSharing = false; // LC-408
      // Record ourselves so the preview is correct if we leave before any
      // VoiceJoined echoes back; the roster event will overwrite this.
      participants[cfg.selfId] = cfg.selfName;
      showJoinedUi(true);
      createTile(cfg.selfId, cfg.selfName, true);
      startSpeaking();
      startQuality();
      var sv = tileVideo(cfg.selfId);
      // Re-assign (null first) so the <video> re-renders with the changed
      // track set - assigning the same stream object can be a no-op.
      if (sv) { sv.srcObject = null; sv.srcObject = localStream; }
      updateTileMedia(cfg.selfId);
      setCameraBtn();
      wsSend({ type: 'voice_join', room_id: cfg.roomId });
      // LC-393 Phase 2: publish the channel room so transcribe.js can open a
      // transcription session, and signal that we are in a live call.
      // LC-613: one shared session-room global + lifecycle event across the call
      // and huddle surfaces (was __lcVoiceRoom + lc:voice-joined).
      window.__lcSessionRoom = cfg.roomId;
      try { document.dispatchEvent(new CustomEvent('lc:rtc-session-started', { detail: { surface: 'voice', room: cfg.roomId } })); } catch (e) {}
    }).catch(function () {
      alert(window.__lcS('callNoMic', 'Could not access your microphone.'));
    });
  }

  // LC-610: the SFU join. Shares presence, the tile grid, and the control
  // buttons with the mesh path, but hands the media to huddle_sfu.js. The WS
  // `voice_join` still fires - it is what registers us in the hub roster (which
  // the token endpoint gates on) and what drives the ring + transcription gate.
  function joinSfu() {
    joined = true;
    sfu = true;
    selfMuted = false;
    selfSharing = false;
    sfuCamOn = false;
    participants[cfg.selfId] = cfg.selfName;
    showJoinedUi(true);
    createTile(cfg.selfId, cfg.selfName, true);
    setCameraBtn();
    // Presence first, so by the time huddle_sfu.js requests a token the server
    // has us in the roster. The token fetch retries to absorb the WS-vs-HTTP
    // ordering, so a small delay here is harmless.
    wsSend({ type: 'voice_join', room_id: cfg.roomId });
    window.__lcSessionRoom = cfg.roomId;
    try { document.dispatchEvent(new CustomEvent('lc:rtc-session-started', { detail: { surface: 'voice', room: cfg.roomId } })); } catch (e) {}
    window.LetsChatHuddleSfu.start(cfg, sfuHooks()).then(function (ok) {
      if (!ok) {
        // Media never came up (SDK, token, or connection). The mesh is not a
        // fallback here - the server told us this room is SFU - so surface it
        // and leave, rather than sit in a call that carries no audio.
        alert(window.__lcS('callConnectionFailed', 'The call connection failed.'));
        leave();
      }
    });
  }

  // The tile/grid callbacks huddle_sfu.js renders through, so an SFU huddle
  // reuses the exact mesh tiles rather than duplicating them.
  function sfuHooks() {
    return {
      selfId: cfg.selfId,
      audioSink: function () {
        var el = document.getElementById('lc-huddle-sfu-audio-sink');
        if (!el) {
          el = document.createElement('div');
          el.id = 'lc-huddle-sfu-audio-sink';
          el.style.display = 'none';
          document.body.appendChild(el);
        }
        return el;
      },
      addTile: function (uid, label) {
        if (!uid || uid === cfg.selfId) return;
        participants[uid] = label;
        createTile(uid, label, false);
        updateCount();
      },
      removeTile: function (uid) {
        if (!uid) return;
        delete participants[uid];
        removeTile(uid);
        updateCount();
      },
      tileVideo: tileVideo,
      setHasVideo: function (uid, on) {
        var v = tileVideo(uid);
        var t = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(uid) + '"]');
        if (!v || !t) return;
        var avatar = t.querySelector('[data-lc-voice-avatar]');
        v.style.display = on ? '' : 'none';
        if (avatar) avatar.style.display = on ? 'none' : '';
      },
      setMuted: applyMute,
      setScreen: applyScreen,
      setSpeaking: function (loud) {
        var g = grid();
        if (!g) return;
        var tiles = g.querySelectorAll('[data-lc-voice-tile]');
        for (var i = 0; i < tiles.length; i++) {
          var id = tiles[i].getAttribute('data-lc-voice-tile');
          tiles[i].setAttribute('data-speaking', loud[id] ? 'true' : 'false');
        }
      },
      // LC-764: huddle_sfu.js calls this when a mute toggle rejects or resolves
      // without flipping the track. Surface it instead of only correcting the
      // button label, so the user knows the change did not take (and peers are
      // not hearing them) rather than believing they are live.
      micError: function () {
        try {
          alert(window.__lcS('callMicToggleFailed',
            'Could not change your microphone. Try muting and unmuting again.'));
        } catch (e) {}
      },
    };
  }

  function leave() {
    if (!joined) return;
    // LC-610: SFU media is owned by huddle_sfu.js; tear it down, then run the
    // shared presence/UI cleanup (the mesh-specific steps below are skipped).
    if (sfu) {
      sfu = false;
      selfMuted = false;
      selfSharing = false;
      sfuCamOn = false;
      window.LetsChatHuddleSfu.stop();
      wsSend({ type: 'voice_leave', room_id: cfg.roomId });
      removeTile(cfg.selfId);
      var gs = grid();
      if (gs) gs.replaceChildren();
      joined = false;
      var sss = q('[data-lc-voice-screen]');
      if (sss) { setLabel(sss, window.__lcS('callShareScreen', 'Share screen')); sss.setAttribute('aria-pressed', 'false'); }
      if (cfg) delete participants[cfg.selfId];
      renderPreview();
      showJoinedUi(false);
      window.__lcSessionRoom = null;
      try { document.dispatchEvent(new CustomEvent('lc:rtc-session-ended', { detail: { surface: 'voice' } })); } catch (e) {}
      return;
    }
    selfMuted = false;
    selfSharing = false;
    stopSpeaking();
    stopQuality();
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
    if (ss) { setLabel(ss, window.__lcS('callShareScreen', 'Share screen')); ss.setAttribute('aria-pressed', 'false'); }
    // Drop ourselves from the participants map and re-render the preview
    // before un-hiding it. Otherwise the leaver would see whatever was
    // server-rendered at page load, which is often "no one is here yet" even
    // when the call is still busy.
    if (cfg) delete participants[cfg.selfId];
    renderPreview();
    showJoinedUi(false);
    // LC-393 Phase 2: we left the channel - transcribe.js stops our local
    // capture (the session itself continues for whoever is still here).
    // LC-613: `surface: 'voice'` tells transcribe.js the shared session may
    // outlive us, so it stops local capture without finalizing (a 'call' end
    // finalizes, being sole-participant).
    window.__lcSessionRoom = null;
    try { document.dispatchEvent(new CustomEvent('lc:rtc-session-ended', { detail: { surface: 'voice' } })); } catch (e) {}
  }

  // LC-626: tell transcribe.js our mic state so it stops/resumes its own
  // (separate) transcription capture. Muting the RTC track alone silences peers
  // but leaves live transcription recording, so a muted speaker's words still
  // land in the transcript - a privacy leak.
  function emitMicMuted(muted) {
    try { document.dispatchEvent(new CustomEvent('lc:mic-muted', { detail: { muted: !!muted } })); } catch (e) {}
  }

  function toggleMute() {
    // LC-610: on the SFU, huddle_sfu.js owns the mic and reflects the tile +
    // remote peers; just sync the button off the state it returns.
    if (sfu) {
      window.LetsChatHuddleSfu.toggleMute().then(function (muted) {
        selfMuted = muted;
        var b = q('[data-lc-voice-mute]');
        if (b) {
          setLabel(b, muted ? window.__lcS('callUnmute', 'Unmute') : window.__lcS('callMute', 'Mute'));
          b.setAttribute('aria-pressed', muted ? 'true' : 'false');
        }
        emitMicMuted(muted);
      });
      return;
    }
    if (!localStream) return;
    dbg('toggle: before', audioTrackStates());
    var on = true;
    localStream.getAudioTracks().forEach(function (t) { t.enabled = !t.enabled; on = t.enabled; });
    var muted = !on;
    dbg('toggle: after', audioTrackStates());
    selfMuted = muted;
    var btn = q('[data-lc-voice-mute]');
    if (btn) {
      setLabel(btn, on ? window.__lcS('callMute', 'Mute') : window.__lcS('callUnmute', 'Unmute'));
      btn.setAttribute('aria-pressed', muted ? 'true' : 'false'); // LC-402 on/off state
    }
    // LC-402: reflect our own mute on our tile and tell the channel so peers
    // light the mute indicator on our remote tile too.
    applyMute(cfg.selfId, muted);
    wsSend({ type: 'voice_mute', room_id: cfg.roomId, muted: muted });
    emitMicMuted(muted);
  }

  // LC-402: set the mute indicator on a participant's tile (self or remote).
  function applyMute(userId, muted) {
    if (!userId) return;
    var t = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    if (t) t.setAttribute('data-muted', muted ? 'true' : 'false');
  }

  // LC-408: pin/unpin a participant's tile as the screen-share stage (the CSS
  // `:has([data-screen])` grid rule reshapes the layout when any tile is set).
  function applyScreen(userId, sharing) {
    if (!userId) return;
    var t = grid() && grid().querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    if (!t) return;
    if (sharing) t.setAttribute('data-screen', 'true');
    else t.removeAttribute('data-screen');
  }
  // Mark our own tile and tell the channel so peers pin our share too.
  function announceScreen(sharing) {
    selfSharing = !!sharing;
    applyScreen(cfg.selfId, selfSharing);
    wsSend({ type: 'voice_screen', room_id: cfg.roomId, sharing: selfSharing });
  }

  function setCameraBtn() {
    var btn = q('[data-lc-voice-camera]');
    if (!btn) return;
    var on = sfu ? sfuCamOn : hasLocalCamera();
    setLabel(btn, on ? window.__lcS('callStopVideo', 'Stop video') : window.__lcS('callStartVideo', 'Start video'));
    btn.setAttribute('aria-pressed', on ? 'true' : 'false'); // LC-402 on/off state
    // While the screen-share track owns the video sender on every peer,
    // toggling the camera would race for the same outgoing slot.
    if (isSharingScreen()) btn.setAttribute('disabled', '');
    else btn.removeAttribute('disabled');
  }
  function setScreenBtn() {
    var btn = q('[data-lc-voice-screen]');
    if (!btn) return;
    var on = sfu ? selfSharing : isSharingScreen();
    setLabel(btn, on ? window.__lcS('callStopSharing', 'Stop sharing') : window.__lcS('callShareScreen', 'Share screen'));
    btn.setAttribute('aria-pressed', on ? 'true' : 'false'); // LC-402 on/off state
  }

  // Camera on/off mid-call: acquire or release the video track and add it to
  // (or remove it from) every peer connection, which renegotiates each.
  function toggleCamera() {
    if (sfu) {
      window.LetsChatHuddleSfu.toggleCamera().then(function (on) {
        sfuCamOn = on;
        setCameraBtn();
      });
      return;
    }
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
    if (sfu) {
      window.LetsChatHuddleSfu.toggleScreen().then(function (on) {
        selfSharing = on;
        setScreenBtn();
      });
      return;
    }
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
      announceScreen(true); // LC-408: pin our tile + tell peers
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
    announceScreen(false); // LC-408: unpin our tile + tell peers
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
        // LC-784: pair[2] is the peer's avatar version (roster tuples became
        // triples). Record it before addPeer -> createTile builds the URL.
        if (pair[2]) avatarVersions[pair[0]] = pair[2];
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
      // LC-784: record their avatar version before addPeer/createTile below.
      var joinedVersion = node.getAttribute('data-avatar-version');
      if (userId && joinedVersion) avatarVersions[userId] = joinedVersion;
      renderPreview();
      // Someone joined after us; they will offer, we answer. Skip when the
      // event is about us (we already created our own tile in join()).
      if (joined && userId !== cfg.selfId) {
        addPeer(userId, username, false);
        // LC-402/408: a fresh joiner has no record of our current mic / screen
        // state, so re-announce whatever is active (broadcast reaches them too).
        if (selfMuted) wsSend({ type: 'voice_mute', room_id: cfg.roomId, muted: true });
        if (selfSharing) wsSend({ type: 'voice_screen', room_id: cfg.roomId, sharing: true });
      }
      return;
    }
    if (kind === 'left') {
      if (userId) delete participants[userId];
      renderPreview();
      removePeer(userId);
      return;
    }
    if (kind === 'mute') { // LC-402: a peer toggled their mic
      applyMute(userId, payload === '1');
      return;
    }
    if (kind === 'screen') { // LC-408: a peer started/stopped screen-sharing
      applyScreen(userId, payload === '1');
      // Nudge the camera-vs-avatar swap: on stop the peer often drops their
      // video track, and re-evaluating here reverts the frozen last frame even
      // if the track's own onmute/onremovetrack was missed.
      updateTileMedia(userId);
      return;
    }
    if (!joined) return;
    if (kind === 'offer') onOffer(userId, username, payload);
    else if (kind === 'answer') onAnswer(userId, payload);
    else if (kind === 'ice') onIce(userId, payload);
  }

  // LC-616: the drain loop is shared with call.js (rtc_common.js).
  function watchBus() {
    window.LetsChatRtc.watchBus('lc-voice-bus', 'data-lc-voice-event', handleEvent);
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
      selfVersion: el.getAttribute('data-self-version') || '', // LC-784
      iceServers: el.getAttribute('data-ice-servers') || '[]',
      // LC-610: the server sets this from `livekit::available()`. Absent (an
      // enclave voice channel page, or an older render) reads as mesh.
      sfu: el.getAttribute('data-lc-huddle-sfu') === '1',
    };
    joined = false;
    if (cfg.selfId && cfg.selfVersion) avatarVersions[cfg.selfId] = cfg.selfVersion; // LC-784
    seedParticipantsFromDom();
    applyHuddleVisibility(); // LC-493: reveal the bar if a huddle is already live
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

  // LC-616: delegated control-bar dispatch via the shared binder (rtc_common.js).
  window.LetsChatRtc.bindControls({
    '[data-lc-voice-join]': function () { join(); },
    '[data-lc-voice-leave]': function () { leave(); },
    '[data-lc-voice-mute]': function () { toggleMute(); },
    '[data-lc-voice-camera]': function () { toggleCamera(); },
    '[data-lc-voice-screen]': function () { toggleScreen(); },
  });

  function onReady(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }
  // LC-611: the huddle ring's Join link lands here as /room/{id}?huddle=1.
  // Auto-joining makes Join genuinely one click from wherever the ring was
  // shown, rather than dropping the user on the room to find the huddle bar.
  // Guarded on the bound root's own id so a stale or hand-edited query string
  // cannot join a different room than the one being viewed, and the param is
  // stripped so a refresh (or a shared URL) does not silently re-join.
  function autoJoinFromQuery() {
    if (!root || !cfg || joined) return;
    var params = new URLSearchParams(window.location.search);
    if (params.get('huddle') !== '1') return;
    params.delete('huddle');
    var qs = params.toString();
    try {
      window.history.replaceState({}, '',
        window.location.pathname + (qs ? '?' + qs : '') + window.location.hash);
    } catch (e) {}
    join();
  }

  onReady(function () {
    watchBus();
    scan();
    autoJoinFromQuery();
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
