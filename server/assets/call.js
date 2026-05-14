// 1:1 WebRTC calls, Discord-style.
//
// Persistent-shell script: loaded once per full page load from base.html and
// never re-run by in-place swaps. It watches the OOB "#lc-call-bus" for
// inbound call signals (relayed by the server WS handler) and drives a single
// RTCPeerConnection per open DM page.
//
// There is ONE call type. The voice and video buttons differ only in whether
// the local camera starts enabled - either way the camera can be toggled on
// or off at any point during the call. Signaling rides the existing chat
// WebSocket; the server is a dumb relay forwarding opaque SDP/ICE blobs
// between the two members of a DM room.
//
// Initial setup has no glare: the caller is the sole initiator (it offers,
// the callee builds its RTCPeerConnection lazily from that offer and just
// answers). Mid-call renegotiation - needed when enabling/disabling the
// camera adds or removes a track - uses the "perfect negotiation" pattern so
// either peer can renegotiate; the caller is the impolite peer and the
// callee the polite one, which resolves any offer glare deterministically.
//
// Known v1 limitations: a call is bound to the DM page it started on.
// Navigating away (or the reconnect soft-refresh replacing #main) tears it
// down. Incoming calls only surface when that DM is the page on screen.
(function () {
  'use strict';

  var root = null;        // current [data-lc-call-root] element, or null
  var cfg = null;         // { roomId, selfId, peerName, iceServers }
  var pc = null;          // RTCPeerConnection while a call is live
  var localStream = null;
  var phase = 'idle';     // idle | outgoing | incoming | connecting | in-call
  var polite = false;     // perfect-negotiation role (caller=false, callee=true)
  var makingOffer = false;
  var ignoreOffer = false;
  var incoming = null;    // { fromName } while an invite is ringing
  var pendingCandidates = [];

  // ---- websocket signaling -----------------------------------------
  function signal(kind, payload) {
    var ws = window.__lcWS;
    if (!ws || !cfg) return;
    try {
      ws.send(JSON.stringify({
        type: 'call_signal',
        room_id: cfg.roomId,
        kind: kind,
        payload: payload || null,
      }));
    } catch (e) { /* socket mid-reconnect; the call will time out */ }
  }

  // ---- dom helpers --------------------------------------------------
  function q(sel) { return root ? root.querySelector(sel) : null; }
  function show(el) { if (el) el.classList.remove('hidden'); }
  function hide(el) { if (el) el.classList.add('hidden'); }
  function setStatus(text) { var s = q('[data-lc-call-status]'); if (s) s.textContent = text; }

  function hasLocalVideo() {
    return !!(localStream && localStream.getVideoTracks().length > 0);
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
  // otherwise fall back to their avatar (/avatars/{id} always resolves,
  // default avatar included).
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
    if (btn) btn.textContent = hasLocalVideo() ? 'Stop video' : 'Start video';
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
  function teardown() {
    closePc();
    stopLocal();
    pendingCandidates = [];
    incoming = null;
    phase = 'idle';
    polite = false;
    makingOffer = false;
    ignoreOffer = false;
    hide(q('[data-lc-call-incoming]'));
    hide(q('[data-lc-call-active]'));
    var rv = q('[data-lc-remote-video]'); if (rv) rv.srcObject = null;
    var lv = q('[data-lc-local-video]'); if (lv) lv.srcObject = null;
    var mute = q('[data-lc-call-mute]'); if (mute) mute.textContent = 'Mute';
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
    var iceServers;
    try { iceServers = JSON.parse(cfg.iceServers); }
    catch (e) { iceServers = [{ urls: 'stun:stun.l.google.com:19302' }]; }
    pc = new RTCPeerConnection({ iceServers: iceServers });

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
        setStatus(cfg.peerName);
      } else if (pc.connectionState === 'failed') {
        setStatus('Connection failed');
        endCall();
      } else if (pc.connectionState === 'disconnected') {
        setStatus('Reconnecting...');
      }
    };
  }

  // Attach the current local tracks to the peer connection. Called at the
  // point in each role's flow that keeps the caller the sole initiator of
  // the initial negotiation - the caller right after createPc(), the callee
  // only after it has applied the caller's offer.
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
    // The callee builds its peer connection lazily from the caller's first
    // offer, so the caller alone initiates the setup negotiation (no glare).
    var firstOffer = !pc;
    if (firstOffer) {
      if (phase === 'idle') return;
      createPc();
    }
    var collision = makingOffer || pc.signalingState !== 'stable';
    ignoreOffer = !polite && collision;
    if (ignoreOffer) return;
    // setRemoteDescription performs an implicit rollback when the polite
    // peer is mid-offer, so no explicit rollback call is needed.
    pc.setRemoteDescription(desc)
      .then(function () {
        // Attach the callee's local tracks to the just-offered transceivers
        // before answering, so the answer advertises them.
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
    // Buffer until the connection exists and has a remote description; the
    // callee may receive ICE before it has built its pc from the offer.
    if (!pc || !pc.remoteDescription) { pendingCandidates.push(cand); return; }
    pc.addIceCandidate(cand).catch(function (e) {
      if (!ignoreOffer) console.warn('call: addIceCandidate failed', e);
    });
  }

  // ---- call lifecycle ----------------------------------------------
  function becomeCaller() {
    phase = 'outgoing';
    polite = false; // caller is the impolite peer
    hide(q('[data-lc-call-incoming]'));
    setStatus('Calling ' + cfg.peerName + '...');
    signal('invite', hasLocalVideo() ? 'video' : 'audio');
  }

  // Flip from "calling" to "answering" when glare resolves against us: we
  // already hold local media with the overlay up, so just switch roles and
  // accept the peer's in-flight invite.
  function becomeCallee() {
    phase = 'connecting';
    polite = true; // callee is the polite peer
    hide(q('[data-lc-call-incoming]'));
    setStatus('Connecting...');
    signal('accept');
  }

  function startCall(withVideo) {
    if (phase !== 'idle' || !cfg) return;
    getMedia(withVideo).then(function (stream) {
      localStream = stream;
      refreshLocalVideo();
      refreshRemoteVideo();
      setCameraBtn();
      show(q('[data-lc-call-active]'));
      // An invite from the same peer can land while getUserMedia resolves
      // (both sides calling at once). Resolve deterministically by user id
      // so exactly one side ends up the caller.
      if (phase === 'incoming' && incoming) {
        if (cfg.selfId < incoming.fromId) becomeCaller();
        else becomeCallee();
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
    if (phase !== 'incoming') return;
    // The callee inherits the call's default: a video call starts both peers
    // with the camera on, a voice call with it off. Either peer can flip
    // their own camera at any time with the in-call control.
    var withVideo = incoming ? incoming.withVideo : false;
    getMedia(withVideo).then(function (stream) {
      localStream = stream;
      phase = 'connecting';
      polite = true; // callee is the polite peer
      hide(q('[data-lc-call-incoming]'));
      refreshLocalVideo();
      refreshRemoteVideo();
      setCameraBtn();
      show(q('[data-lc-call-active]'));
      setStatus('Connecting...');
      // The peer connection is built lazily when the caller's offer arrives
      // (see onOffer), keeping the caller the sole initiator at setup.
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
    addLocalTracks(); // -> negotiationneeded -> caller sends the initial offer
  }

  function toggleMute() {
    if (!localStream) return;
    var on = true;
    localStream.getAudioTracks().forEach(function (t) { t.enabled = !t.enabled; on = t.enabled; });
    var btn = q('[data-lc-call-mute]');
    if (btn) btn.textContent = on ? 'Mute' : 'Unmute';
  }

  // Discord-style camera control: available throughout the call regardless of
  // whether it began as a voice or video call. Turning the camera on acquires
  // it and adds a track; turning it off stops and removes the track. Either
  // direction triggers renegotiation via onnegotiationneeded.
  function toggleCamera() {
    if (phase === 'idle' || !localStream) return;
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
        // The call may have ended while the permission prompt was open.
        if (phase === 'idle' || !localStream) { try { vtrack.stop(); } catch (e) {} return; }
        localStream.addTrack(vtrack);
        if (pc) pc.addTrack(vtrack, localStream);
        refreshLocalVideo();
        setCameraBtn();
      }).catch(function () { alert('Could not access your camera.'); });
    }
  }

  // ---- inbound signal dispatch -------------------------------------
  function handleSignal(node) {
    var roomId = node.getAttribute('data-room-id');
    var fromId = node.getAttribute('data-from-id');
    var fromName = node.getAttribute('data-from-name') || 'Someone';
    var kind = node.getAttribute('data-kind');
    var payload = node.getAttribute('data-payload');

    if (!cfg) return;

    // Self-echo: a control signal the server mirrored to our other tabs.
    // Used only to dismiss a ringing prompt that another device answered.
    if (fromId === cfg.selfId) {
      if (phase === 'incoming' &&
          (kind === 'accept' || kind === 'reject' || kind === 'cancel' || kind === 'hangup')) {
        teardown();
      }
      return;
    }

    // Only the DM currently on screen can host a call in v1.
    if (String(cfg.roomId) !== String(roomId)) return;

    switch (kind) {
      case 'invite':
        if (phase === 'idle') {
          incoming = { fromName: fromName, fromId: fromId, withVideo: payload === 'video' };
          var nm = q('[data-lc-call-incoming-name]'); if (nm) nm.textContent = fromName;
          phase = 'incoming';
          show(q('[data-lc-call-incoming]'));
          return;
        }
        if (phase === 'outgoing') {
          // Glare: we are calling them and they are calling us. The lower
          // user id stays the caller; the other side becomes the callee.
          if (cfg.selfId < fromId) return; // we win - keep our outgoing call
          becomeCallee();                  // we lose - accept their call
          return;
        }
        signal('reject'); // genuinely busy in another call
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
  // Bind to the DM page currently in the DOM. Each navigation swaps in a
  // fresh root node, so a non-identical element means the page changed:
  // end any live call (notifying the peer) before switching context.
  function bindRoot(el) {
    if (el === root) return;
    if (phase !== 'idle') endCall();
    root = el;
    cfg = {
      roomId: parseInt(el.getAttribute('data-room-id'), 10),
      selfId: el.getAttribute('data-self-id'),
      peerName: el.getAttribute('data-peer-name') || 'your contact',
      iceServers: el.getAttribute('data-ice-servers') || '[]',
    };
  }

  function scan() {
    var el = document.querySelector('[data-lc-call-root]');
    if (el) {
      bindRoot(el);
    } else if (root) {
      if (phase !== 'idle') endCall();
      teardown();
      root = null;
      cfg = null;
    }
  }

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

  // Delegated click handling: the call buttons (header start buttons and the
  // overlay controls) are re-rendered by htmx swaps, so a single permanent
  // listener is simpler and swap-proof.
  document.body.addEventListener('click', function (e) {
    var t = e.target;
    if (!t || !t.closest) return;
    var b = t.closest('[data-lc-call-start]');
    if (b) { e.preventDefault(); startCall(b.getAttribute('data-lc-call-start') === 'video'); return; }
    if (t.closest('[data-lc-call-accept]')) { acceptCall(); return; }
    if (t.closest('[data-lc-call-decline]')) { declineCall(); return; }
    if (t.closest('[data-lc-call-hangup]')) { endCall(); return; }
    if (t.closest('[data-lc-call-mute]')) { toggleMute(); return; }
    if (t.closest('[data-lc-call-camera]')) { toggleCamera(); return; }
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
})();
