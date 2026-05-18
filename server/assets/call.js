// 1:1 WebRTC calls, Discord-style.
//
// Page-agnostic: the call UI lives in a single global #lc-call-root rendered
// by layout.html, and this script handles invites from any page (so an
// incoming call shows up whether or not the recipient is viewing the DM
// with the caller). Outgoing calls are started from per-DM header buttons
// that carry the room/peer context in data attributes.
//
// Signaling rides the existing chat WebSocket. The server resolves glare
// (both peers inviting simultaneously) per DM room: the first invite wins
// and the loser is delivered the winner's invite, so this script no longer
// needs to compare user ids to pick a caller.
(function () {
  'use strict';

  var selfId = null;          // viewer's user id, set on init from layout
  var iceServers = null;      // resolved RTCIceServer array, fetched lazily
  var iceFetch = null;        // de-dupes concurrent /call/config fetches

  // Per-active-call context. Set on outgoing start or first incoming invite;
  // cleared by teardown(). selfId/iceServers are session-wide and survive.
  var roomId = null;
  var peerId = null;
  var peerName = null;

  var pc = null;              // RTCPeerConnection while a call is live
  var localStream = null;
  var phase = 'idle';         // idle | outgoing | incoming | connecting | in-call
  var polite = false;         // perfect-negotiation role (caller=false, callee=true)
  var makingOffer = false;
  var ignoreOffer = false;
  var incoming = null;        // { fromId, fromName, withVideo } while invite is ringing
  var pendingCandidates = [];

  // Screen-share state. While sharing, screenTrack is the live display capture
  // and replaces the camera in the peer connection's video sender. cameraTrack
  // (if any) is stopped so the OS releases the webcam; restoreCameraAfterShare
  // remembers that we should re-acquire it when sharing ends.
  var screenTrack = null;
  var restoreCameraAfterShare = false;

  // ---- websocket signaling -----------------------------------------
  function signal(kind, payload) {
    var ws = window.__lcWS;
    if (!ws || roomId == null) return;
    try {
      ws.send(JSON.stringify({
        type: 'call_signal',
        room_id: roomId,
        kind: kind,
        payload: payload || null,
      }));
    } catch (e) { /* socket mid-reconnect; the call will time out */ }
  }

  // ---- dom helpers --------------------------------------------------
  function root() { return document.getElementById('lc-call-root'); }
  function q(sel) { var r = root(); return r ? r.querySelector(sel) : null; }
  // show/hide mirror the visual hidden-class onto aria-hidden so the
  // accessibility tree matches. `display:none` already removes the element
  // from the a11y tree, but keeping the attribute in sync is belt-and-
  // suspenders against any future style change. Only call-dialog elements
  // pass through these helpers; safe to apply globally.
  function show(el) { if (el) { el.classList.remove('hidden'); el.setAttribute('aria-hidden', 'false'); } }
  function hide(el) { if (el) { el.classList.add('hidden'); el.setAttribute('aria-hidden', 'true'); } }
  function setStatus(text) { var s = q('[data-lc-call-status]'); if (s) s.textContent = text; }

  // ---- focus trap (modal dialog discipline) -------------------------
  // Exactly one trap is active at a time. installDialogTrap migrates from
  // an existing trap (incoming -> active on Accept) without restoring focus
  // to the pre-dialog source; disposeDialogTrap (called only from teardown,
  // the close funnel) restores focus to whatever element opened the dialog.
  //
  // Listener-cleanup discipline: every call path that closes a dialog ends
  // at teardown(); teardown calls disposeDialogTrap exactly once; dispose
  // calls removeEventListener exactly once. Migrations dispose the previous
  // trap inline so we never hold two listeners simultaneously.
  var currentTrap = null;
  var trapPrevious = null;
  var TRAP_FOCUSABLE = 'button:not([disabled]),[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';
  function installDialogTrap(dialogEl) {
    if (!dialogEl) return;
    if (currentTrap) {
      currentTrap.dispose();
      currentTrap = null;
    } else {
      trapPrevious = document.activeElement;
    }
    var trap = window.__lcDialogTrap;
    currentTrap = trap ? trap(dialogEl) : { dispose: function () {} };
    var first = dialogEl.querySelector(TRAP_FOCUSABLE);
    if (first) { try { first.focus(); } catch (e) {} }
  }
  function disposeDialogTrap() {
    if (currentTrap) { currentTrap.dispose(); currentTrap = null; }
    if (trapPrevious && document.contains(trapPrevious) && typeof trapPrevious.focus === 'function') {
      try { trapPrevious.focus(); } catch (e) {}
    }
    trapPrevious = null;
  }

  function setRemoteAvatar() {
    var av = q('[data-lc-remote-avatar]');
    if (av && peerId != null) {
      av.setAttribute('src', '/avatars/' + peerId);
      av.setAttribute('alt', peerName || '');
    }
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
  function refreshLocalVideo() {
    var lv = q('[data-lc-local-video]');
    var av = q('[data-lc-local-avatar]');
    var on = hasLocalVideo();
    if (lv) {
      lv.srcObject = on ? localStream : null;
      lv.style.display = on ? '' : 'none';
    }
    if (av) av.style.display = on ? 'none' : '';
  }
  // Show the peer's camera when their stream carries a live video track;
  // otherwise fall back to their avatar.
  function refreshRemoteVideo() {
    var rv = q('[data-lc-remote-video]');
    var av = q('[data-lc-remote-avatar]');
    var stream = rv && rv.srcObject;
    var on = !!(stream && stream.getVideoTracks().some(function (t) {
      return t.readyState === 'live' && !t.muted;
    }));
    if (rv) rv.style.display = on ? '' : 'none';
    if (av) av.style.display = on ? 'none' : '';
  }
  function setCameraBtn() {
    var btn = q('[data-lc-call-camera]');
    if (!btn) return;
    var on = hasLocalCamera();
    btn.textContent = on ? 'Stop video' : 'Start video';
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
    // While the screen-share track owns the video sender, toggling the camera
    // would race for the same outgoing slot. The `disabled:*` classes on the
    // button render the disabled affordance.
    if (isSharingScreen()) btn.setAttribute('disabled', '');
    else btn.removeAttribute('disabled');
  }
  function setScreenBtn() {
    var btn = q('[data-lc-call-screen]');
    if (!btn) return;
    var on = isSharingScreen();
    btn.textContent = on ? 'Stop sharing' : 'Share screen';
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
  }

  // ---- teardown -----------------------------------------------------
  function stopLocal() {
    if (localStream) {
      localStream.getTracks().forEach(function (t) { try { t.stop(); } catch (e) {} });
      localStream = null;
    }
  }
  function closePc() {
    if (pc) {
      pc.ontrack = pc.onicecandidate = pc.onnegotiationneeded = pc.onconnectionstatechange = null;
      try { pc.close(); } catch (e) {}
      pc = null;
    }
  }
  function stopScreen() {
    if (screenTrack) {
      screenTrack.onended = null;
      try { screenTrack.stop(); } catch (e) {}
      screenTrack = null;
    }
    restoreCameraAfterShare = false;
  }
  function teardown() {
    closePc();
    stopScreen();
    stopLocal();
    pendingCandidates = [];
    incoming = null;
    phase = 'idle';
    polite = false;
    makingOffer = false;
    ignoreOffer = false;
    roomId = null;
    peerId = null;
    peerName = null;
    hide(q('[data-lc-call-incoming]'));
    hide(q('[data-lc-call-active]'));
    var rv = q('[data-lc-remote-video]'); if (rv) rv.srcObject = null;
    var lv = q('[data-lc-local-video]'); if (lv) lv.srcObject = null;
    var mute = q('[data-lc-call-mute]');
    if (mute) { mute.textContent = 'Mute'; mute.setAttribute('aria-pressed', 'false'); }
    var cam = q('[data-lc-call-camera]');
    if (cam) {
      cam.textContent = 'Start video';
      cam.setAttribute('aria-pressed', 'false');
      cam.removeAttribute('disabled');
    }
    var ss = q('[data-lc-call-screen]');
    if (ss) { ss.textContent = 'Share screen'; ss.setAttribute('aria-pressed', 'false'); }
    disposeDialogTrap();
  }

  // ---- ice config ---------------------------------------------------
  // The ICE-server list is per-deployment (env var) and is fetched once over
  // a small JSON endpoint instead of being threaded into every page struct
  // that extends the layout template.
  function ensureIce() {
    if (iceServers) return Promise.resolve(iceServers);
    if (iceFetch) return iceFetch;
    iceFetch = fetch('/call/config', { credentials: 'same-origin' })
      .then(function (r) { return r.ok ? r.json() : Promise.reject(); })
      .then(function (j) {
        iceServers = Array.isArray(j.iceServers) ? j.iceServers
          : [{ urls: 'stun:stun.l.google.com:19302' }];
        return iceServers;
      })
      .catch(function () {
        iceServers = [{ urls: 'stun:stun.l.google.com:19302' }];
        iceFetch = null;
        return iceServers;
      });
    return iceFetch;
  }

  // ---- media + peer connection -------------------------------------
  function getMedia(withVideo) {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      return Promise.reject(new Error('getUserMedia unavailable'));
    }
    return navigator.mediaDevices.getUserMedia(
      withVideo ? { audio: true, video: true } : { audio: true }
    );
  }

  function createPc() {
    pc = new RTCPeerConnection({ iceServers: iceServers || [] });

    pc.onicecandidate = function (ev) {
      if (ev.candidate) signal('ice', JSON.stringify(ev.candidate));
    };
    pc.ontrack = function (ev) {
      var rv = q('[data-lc-remote-video]');
      if (rv && ev.streams && ev.streams[0]) {
        rv.srcObject = ev.streams[0];
        ev.streams[0].onaddtrack = function () { refreshRemoteVideo(); };
        ev.streams[0].onremovetrack = function () { refreshRemoteVideo(); };
      }
      if (ev.track) {
        ev.track.onmute = function () { refreshRemoteVideo(); };
        ev.track.onunmute = function () { refreshRemoteVideo(); };
        ev.track.onended = function () { refreshRemoteVideo(); };
      }
      refreshRemoteVideo();
    };
    // Perfect negotiation: either peer may fire this (e.g. after a camera
    // toggle adds/removes a track). Glare is resolved on the offer side.
    pc.onnegotiationneeded = function () {
      makingOffer = true;
      pc.setLocalDescription()
        .then(function () { signal('offer', JSON.stringify(pc.localDescription)); })
        .catch(function (e) { console.warn('call: negotiation failed', e); })
        .finally(function () { makingOffer = false; });
    };
    pc.onconnectionstatechange = function () {
      if (!pc) return;
      if (pc.connectionState === 'connected') {
        phase = 'in-call';
        setStatus(peerName || '');
      } else if (pc.connectionState === 'failed') {
        setStatus('Connection failed');
        endCall();
      } else if (pc.connectionState === 'disconnected') {
        setStatus('Reconnecting...');
      }
    };
  }

  function addLocalTracks() {
    if (!pc || !localStream) return;
    localStream.getTracks().forEach(function (t) { pc.addTrack(t, localStream); });
  }

  function flushCandidates() {
    var list = pendingCandidates;
    pendingCandidates = [];
    list.forEach(function (c) {
      pc.addIceCandidate(c).catch(function (e) {
        if (!ignoreOffer) console.warn('call: addIceCandidate failed', e);
      });
    });
  }

  function onOffer(sdp) {
    var desc;
    try { desc = JSON.parse(sdp); } catch (e) { return; }
    var firstOffer = !pc;
    if (firstOffer) {
      if (phase === 'idle') return;
      createPc();
    }
    var collision = makingOffer || pc.signalingState !== 'stable';
    ignoreOffer = !polite && collision;
    if (ignoreOffer) return;
    pc.setRemoteDescription(desc)
      .then(function () {
        if (firstOffer) addLocalTracks();
        flushCandidates();
        return pc.setLocalDescription();
      })
      .then(function () { signal('answer', JSON.stringify(pc.localDescription)); })
      .catch(function (e) { console.warn('call: onOffer failed', e); });
  }

  function onAnswer(sdp) {
    if (!pc) return;
    var desc;
    try { desc = JSON.parse(sdp); } catch (e) { return; }
    pc.setRemoteDescription(desc)
      .then(flushCandidates)
      .catch(function (e) { console.warn('call: onAnswer failed', e); });
  }

  function onIce(payload) {
    if (!payload) return;
    var cand;
    try { cand = JSON.parse(payload); } catch (e) { return; }
    if (!pc || !pc.remoteDescription) { pendingCandidates.push(cand); return; }
    pc.addIceCandidate(cand).catch(function (e) {
      if (!ignoreOffer) console.warn('call: addIceCandidate failed', e);
    });
  }

  // ---- call lifecycle ----------------------------------------------
  function becomeCaller() {
    phase = 'outgoing';
    polite = false;
    hide(q('[data-lc-call-incoming]'));
    setStatus('Calling ' + (peerName || '') + '...');
    signal('invite', hasLocalVideo() ? 'video' : 'audio');
  }

  function becomeCallee() {
    phase = 'connecting';
    polite = true;
    hide(q('[data-lc-call-incoming]'));
    setStatus('Connecting...');
    signal('accept');
  }

  function startCall(withVideo, ctx) {
    if (phase !== 'idle' && phase !== 'incoming') return;
    // Outgoing context comes from the clicked button; if a call is already
    // bound (we are mid-incoming for the same peer) keep the existing one.
    if (phase === 'idle') {
      roomId = ctx.roomId;
      peerId = ctx.peerId;
      peerName = ctx.peerName;
      setRemoteAvatar();
    }
    Promise.all([ensureIce(), getMedia(withVideo)])
      .then(function (results) {
        localStream = results[1];
        refreshLocalVideo();
        refreshRemoteVideo();
        setCameraBtn();
        show(q('[data-lc-call-active]'));
        installDialogTrap(q('[data-lc-call-active]'));
        // The server resolves glare; if an invite arrived while we were
        // acquiring media the phase has already flipped to 'incoming'.
        // Honor that: we are now the callee.
        if (phase === 'incoming' && incoming) {
          becomeCallee();
          incoming = null;
          return;
        }
        if (phase !== 'idle') { teardown(); return; }
        becomeCaller();
      }).catch(function () {
        alert('Could not access your microphone or camera.');
        teardown();
      });
  }

  function acceptCall() {
    if (phase !== 'incoming' || !incoming) return;
    // Drop any active enclave voice channel first - the OS only has one
    // mic/camera and the user can't be in two calls at once.
    if (window.LetsChatVoice && window.LetsChatVoice.isJoined()) {
      window.LetsChatVoice.leave();
    }
    var withVideo = incoming.withVideo;
    Promise.all([ensureIce(), getMedia(withVideo)])
      .then(function (results) {
        localStream = results[1];
        phase = 'connecting';
        polite = true;
        hide(q('[data-lc-call-incoming]'));
        refreshLocalVideo();
        refreshRemoteVideo();
        setCameraBtn();
        show(q('[data-lc-call-active]'));
        installDialogTrap(q('[data-lc-call-active]'));
        setStatus('Connecting...');
        signal('accept');
        incoming = null;
      }).catch(function () {
        alert('Could not access your microphone or camera.');
        signal('reject');
        teardown();
      });
  }

  function declineCall() {
    if (phase !== 'incoming') return;
    signal('reject');
    teardown();
  }

  function endCall() {
    if (phase === 'idle') return;
    signal(phase === 'outgoing' ? 'cancel' : 'hangup');
    teardown();
  }

  function onAccept() {
    if (phase !== 'outgoing') return;
    phase = 'connecting';
    setStatus('Connecting...');
    createPc();
    addLocalTracks();
  }

  function toggleMute() {
    if (!localStream) return;
    var on = true;
    localStream.getAudioTracks().forEach(function (t) { t.enabled = !t.enabled; on = t.enabled; });
    var btn = q('[data-lc-call-mute]');
    if (btn) {
      btn.textContent = on ? 'Mute' : 'Unmute';
      // aria-pressed="true" means "currently muted" (the toggle is on).
      // Mirrors the visual: button reads "Unmute" exactly when muted.
      btn.setAttribute('aria-pressed', on ? 'false' : 'true');
    }
  }

  function toggleCamera() {
    if (phase === 'idle' || !localStream) return;
    if (isSharingScreen()) return;  // setCameraBtn disables the control; guard the dispatch path too.
    var existing = localStream.getVideoTracks()[0];
    if (existing) {
      if (pc) {
        var sender = pc.getSenders().find(function (s) { return s.track === existing; });
        if (sender) pc.removeTrack(sender);
      }
      existing.stop();
      localStream.removeTrack(existing);
      refreshLocalVideo();
      setCameraBtn();
    } else {
      navigator.mediaDevices.getUserMedia({ video: true }).then(function (vstream) {
        var vtrack = vstream.getVideoTracks()[0];
        if (!vtrack) return;
        if (phase === 'idle' || !localStream) { try { vtrack.stop(); } catch (e) {} return; }
        localStream.addTrack(vtrack);
        if (pc) pc.addTrack(vtrack, localStream);
        refreshLocalVideo();
        setCameraBtn();
      }).catch(function () { alert('Could not access your camera.'); });
    }
  }

  // Find the PeerConnection sender that currently transports our outgoing
  // video track (camera or screen). Returns null if no video has ever been
  // negotiated. Walks transceivers because `removeTrack` leaves the sender
  // in place with a null track, and we still want to reuse that slot.
  function findVideoSender() {
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

  // Drop the screen-capture track, restore the camera if it was running when
  // sharing started. Shared by the manual button, the browser-native "Stop
  // sharing" widget (track.onended), and teardown indirectly via stopScreen.
  function endScreenShare() {
    if (!isSharingScreen()) return;
    var sender = pc ? pc.getSenders().find(function (s) { return s.track === screenTrack; }) : null;
    var wantCamera = restoreCameraAfterShare;
    stopScreen();
    // Swap the outgoing slot back to camera (if it was on) or null it out.
    function applyCameraTrack(track) {
      if (sender) {
        sender.replaceTrack(track).catch(function (e) { console.warn('call: replaceTrack failed', e); });
      } else if (track && pc && localStream) {
        pc.addTrack(track, localStream);
      }
    }
    if (wantCamera) {
      navigator.mediaDevices.getUserMedia({ video: true }).then(function (vstream) {
        var vtrack = vstream.getVideoTracks()[0];
        if (!vtrack) return;
        if (phase === 'idle' || !localStream) { try { vtrack.stop(); } catch (e) {} return; }
        localStream.addTrack(vtrack);
        applyCameraTrack(vtrack);
        refreshLocalVideo();
        setCameraBtn();
        setScreenBtn();
      }).catch(function () {
        // Camera unavailable on the way back. Clear the sender so the peer
        // does not keep rendering the last frame of our screen share.
        applyCameraTrack(null);
        refreshLocalVideo();
        setCameraBtn();
        setScreenBtn();
      });
    } else {
      applyCameraTrack(null);
      refreshLocalVideo();
      setCameraBtn();
      setScreenBtn();
    }
  }

  function toggleScreen() {
    if (phase === 'idle' || !localStream) return;
    if (isSharingScreen()) { endScreenShare(); return; }
    if (!navigator.mediaDevices || !navigator.mediaDevices.getDisplayMedia) {
      alert('Screen sharing is not supported by your browser.');
      return;
    }
    if (!pc) return;
    navigator.mediaDevices.getDisplayMedia({ video: true, audio: false }).then(function (dstream) {
      var dtrack = dstream.getVideoTracks()[0];
      if (!dtrack) return;
      if (phase === 'idle' || !localStream || !pc) {
        try { dtrack.stop(); } catch (e) {}
        return;
      }
      // Hint the encoder that this is mostly static content with sharp edges.
      // Browsers that honor this bias toward higher resolution / lower frame
      // rate, which is what screen content actually wants.
      try { dtrack.contentHint = 'detail'; } catch (e) {}
      var camera = localStream.getVideoTracks()[0];
      restoreCameraAfterShare = !!camera;
      // Finish the swap once the outgoing track is in place on the peer
      // connection. Defers stop()ing the camera until then so the sender does
      // not race with a half-completed replaceTrack.
      function finalize() {
        if (camera) {
          try { camera.stop(); } catch (e) {}
          try { localStream.removeTrack(camera); } catch (e) {}
        }
        localStream.addTrack(dtrack);
        screenTrack = dtrack;
        // User clicks "Stop sharing" in the browser chrome -> end the share.
        dtrack.onended = function () { endScreenShare(); };
        refreshLocalVideo();
        setCameraBtn();
        setScreenBtn();
      }
      // Fallback path: drop the camera sender entirely and addTrack the screen
      // track, which forces onnegotiationneeded -> new SDP -> peer receives
      // the new track. Used both when there is no existing video sender and
      // when replaceTrack rejected (codec/parameter mismatch).
      function addTrackAndFinalize(existingSender) {
        if (existingSender) {
          try { pc.removeTrack(existingSender); } catch (e) {}
        }
        try { pc.addTrack(dtrack, localStream); }
        catch (e) {
          console.warn('call: addTrack(screen) failed', e);
          try { dtrack.stop(); } catch (ee) {}
          return;
        }
        finalize();
      }
      var sender = findVideoSender();
      if (sender && sender.track) {
        // Fast path: keep the same transceiver, no renegotiation. If the
        // browser rejects the swap (rare, but reported for getDisplayMedia
        // tracks whose codec/parameters do not match the camera's), fall
        // back to a fresh addTrack so the peer actually receives the share.
        sender.replaceTrack(dtrack).then(finalize).catch(function (e) {
          console.warn('call: replaceTrack failed, falling back to addTrack', e);
          addTrackAndFinalize(sender);
        });
      } else {
        addTrackAndFinalize(sender);
      }
    }).catch(function (e) {
      // User-cancel from the picker lands here too; nothing to undo.
      if (e && e.name !== 'NotAllowedError') console.warn('call: getDisplayMedia failed', e);
    });
  }

  // ---- inbound signal dispatch -------------------------------------
  function handleSignal(node) {
    var msgRoomId = parseInt(node.getAttribute('data-room-id'), 10);
    var fromId = node.getAttribute('data-from-id');
    var fromName = node.getAttribute('data-from-name') || 'Someone';
    var kind = node.getAttribute('data-kind');
    var payload = node.getAttribute('data-payload');

    // Self-echo: a control signal the server mirrored to our other tabs.
    // Used only to dismiss a ringing prompt that another device answered.
    if (fromId === selfId) {
      if (phase === 'incoming' &&
          (kind === 'accept' || kind === 'reject' || kind === 'cancel' || kind === 'hangup')) {
        teardown();
      }
      return;
    }

    // Once a call is active for a specific DM, ignore any signals tagged for
    // a different DM (a stray invite from a third party while we're busy).
    if (roomId != null && msgRoomId !== roomId && kind !== 'invite') return;

    switch (kind) {
      case 'invite':
        if (phase === 'idle') {
          roomId = msgRoomId;
          peerId = fromId;
          peerName = fromName;
          setRemoteAvatar();
          incoming = { fromId: fromId, fromName: fromName, withVideo: payload === 'video' };
          var nm = q('[data-lc-call-incoming-name]'); if (nm) nm.textContent = fromName;
          phase = 'incoming';
          show(q('[data-lc-call-incoming]'));
          installDialogTrap(q('[data-lc-call-incoming]'));
          return;
        }
        if (phase === 'outgoing' && msgRoomId === roomId && fromId === peerId) {
          // Server-resolved glare loss: the peer was already ringing us
          // (or won the race) and the server replayed their invite back
          // to us. Drop our outgoing UI and accept their call.
          incoming = { fromId: fromId, fromName: fromName, withVideo: payload === 'video' };
          becomeCallee();
          incoming = null;
          return;
        }
        // Already busy with another peer: politely reject so the new
        // caller's UI tears down rather than ringing indefinitely.
        try {
          var ws = window.__lcWS;
          if (ws) ws.send(JSON.stringify({
            type: 'call_signal', room_id: msgRoomId, kind: 'reject', payload: null,
          }));
        } catch (e) {}
        return;
      case 'accept':
        onAccept();
        break;
      case 'reject':
        if (phase === 'outgoing') { setStatus('Call declined'); setTimeout(teardown, 1200); }
        break;
      case 'cancel':
        if (phase === 'incoming') teardown();
        break;
      case 'hangup':
        if (phase !== 'idle') { setStatus('Call ended'); setTimeout(teardown, 800); }
        break;
      case 'offer':
        onOffer(payload);
        break;
      case 'answer':
        onAnswer(payload);
        break;
      case 'ice':
        onIce(payload);
        break;
    }
  }

  // ---- wiring -------------------------------------------------------
  function watchBus() {
    var bus = document.getElementById('lc-call-bus');
    if (!bus) return;
    new MutationObserver(function (muts) {
      muts.forEach(function (m) {
        Array.prototype.forEach.call(m.addedNodes, function (n) {
          if (n.nodeType !== 1) return;
          if (n.hasAttribute('data-lc-call-event')) {
            handleSignal(n);
          } else if (n.querySelector) {
            var c = n.querySelector('[data-lc-call-event]');
            if (c) handleSignal(c);
          }
        });
      });
      bus.replaceChildren();
    }).observe(bus, { childList: true });
  }

  function readSelfId() {
    var el = document.getElementById('lc-call-root');
    if (el) selfId = el.getAttribute('data-self-id');
  }

  // Delegated click handling: the call buttons (DM-header start buttons and
  // the overlay controls) may be re-rendered by htmx swaps, so a single
  // permanent listener is simpler and swap-proof.
  document.body.addEventListener('click', function (e) {
    var t = e.target;
    if (!t || !t.closest) return;
    var startBtn = t.closest('[data-lc-call-start]');
    if (startBtn) {
      e.preventDefault();
      var ctx = {
        roomId: parseInt(startBtn.getAttribute('data-room-id'), 10),
        peerId: startBtn.getAttribute('data-peer-id'),
        peerName: startBtn.getAttribute('data-peer-name') || 'your contact',
      };
      if (!isFinite(ctx.roomId)) return;
      startCall(startBtn.getAttribute('data-lc-call-start') === 'video', ctx);
      return;
    }
    if (t.closest('[data-lc-call-accept]')) { acceptCall(); return; }
    if (t.closest('[data-lc-call-decline]')) { declineCall(); return; }
    if (t.closest('[data-lc-call-hangup]')) { endCall(); return; }
    if (t.closest('[data-lc-call-mute]')) { toggleMute(); return; }
    if (t.closest('[data-lc-call-camera]')) { toggleCamera(); return; }
    if (t.closest('[data-lc-call-screen]')) { toggleScreen(); return; }
  });

  function onReady(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }
  onReady(function () {
    readSelfId();
    watchBus();
  });
})();
