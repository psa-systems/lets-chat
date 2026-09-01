// LC-853 phase 1: huddle remote-control consent flow (request -> Allow/Deny ->
// active banner -> revoke). Consent and state machine only - no input capture
// or replay rides this module; that is the LC-854 transport phase, which plugs
// in underneath without changing this handshake.
//
// The DOM it drives lives in partials/huddle_control.html, included in the
// huddle/voice control bar only while the workspace switch is on (the server
// refuses signals independently - this module is presentation). Signals ride
// the chat WS as `remote_control_signal` frames, exactly like the 1:1 DM flow
// (LC-183); answers come back as data-lc-control-event nodes on #lc-control-bus.
// call.js watches the same bus for its DM calls: both consumers filter by room,
// and a MutationObserver delivers each record to every observer, so the shared
// bus needs no ownership handoff.
//
// Who is sharing is tracked from the same #lc-voice-bus `screen` events that
// pin the tiles (voice.js applyScreen): the request affordance surfaces only
// while joined with exactly ONE other participant sharing - zero or several
// sharers is ambiguous and the server refuses it too (fail closed, both ends).
(function () {
  'use strict';

  // Pure eligibility for the "Request control" affordance; exported for tests.
  // state: { enabled, joined, selfId, sharers: {userId: bool} }.
  function canRequest(state) {
    if (!state.enabled || !state.joined || !state.selfId) return false;
    var ids = Object.keys(state.sharers || {}).filter(function (id) {
      return !!state.sharers[id];
    });
    return ids.length === 1 && ids[0] !== state.selfId;
  }

  window.LetsChatHuddleControl = { canRequest: canRequest };

  // Same requester patience as the 1:1 flow (call.js); the sharer prompt
  // auto-dismisses to Deny well before the server's 45s pending slot expires.
  var REQUEST_TIMEOUT_MS = 12000;
  var PROMPT_AUTODENY_MS = 30000;

  // Requester-side phase: 'idle' | 'requesting' | 'controlling'.
  var phase = 'idle';
  // The user id whose screen we are controlling (the sharer), while controlling.
  var controllingSharer = null;
  // Controller-side: the active capture unbind fn (LC-854), or null.
  var captureStop = null;
  // Sharer-side: the controller holding our grant, and the pending requester.
  var grantedTo = null;
  var pendingFrom = null;
  var pendingFromName = '';
  var sharers = {};
  var requestTimer = null;
  var promptTimer = null;
  // Controller-side: whether the sharer's machine has a native injector
  // (LC-640). null = not yet announced, true/false = known. A web sharer has
  // none, so the controller's input reaches no OS - surface that, do not leave
  // it a mystery.
  var sharerNative = null;

  // The bar can be re-rendered by an htmx page swap (LC-837 nav boost), so
  // never cache elements - resolve through the CURRENT root on every use.
  function root() { return document.querySelector('[data-lc-huddle-control]'); }
  function q(sel) { var r = root(); return r ? r.querySelector(sel) : null; }
  function roomId() {
    var r = root();
    return r ? parseInt(r.getAttribute('data-lc-room'), 10) : NaN;
  }
  function selfId() {
    var cr = document.querySelector('[data-lc-call-root]');
    return cr ? cr.getAttribute('data-self-id') : null;
  }
  function joined() {
    return !!(window.LetsChatVoice && window.LetsChatVoice.isJoined());
  }
  // LC-854: v1 transport is the LiveKit data channel, so control is offered
  // only on SFU huddles. A mesh huddle (no LiveKit configured) has no data
  // channel; the affordance stays hidden there rather than granting a session
  // whose input would go nowhere.
  function sfuHuddle() {
    var vr = document.querySelector('[data-lc-voice-root]');
    return !!vr && vr.getAttribute('data-lc-huddle-sfu') === '1';
  }
  function str(key, fallback) {
    return window.__lcS ? window.__lcS(key, fallback) : fallback;
  }
  function show(el) { if (el) { el.classList.remove('hidden'); el.setAttribute('aria-hidden', 'false'); } }
  function hide(el) { if (el) { el.classList.add('hidden'); el.setAttribute('aria-hidden', 'true'); } }
  function announce(text) {
    var el = q('[data-lc-huddle-control-status]');
    if (el) el.textContent = text || '';
  }
  function send(kind) {
    var ws = window.__lcWS;
    var rid = roomId();
    if (!ws || isNaN(rid)) return;
    try {
      ws.send(JSON.stringify({ type: 'remote_control_signal', room_id: rid, kind: kind }));
    } catch (e) { /* socket reconnecting */ }
  }

  function setRequestBtn() {
    var btn = q('[data-lc-huddle-control-request]');
    if (!btn) return;
    var visible = phase !== 'idle' ||
      canRequest({ enabled: sfuHuddle(), joined: joined(), selfId: selfId(), sharers: sharers });
    btn.classList.toggle('hidden', !visible);
    var text = phase === 'requesting' ? str('callRequesting', 'Requesting...')
      : phase === 'controlling' ? str('callStopControlling', 'Stop controlling')
        : str('callRequestControl', 'Request control');
    var label = btn.querySelector('[data-lc-huddle-control-request-label]');
    if (label) label.textContent = text;
    btn.setAttribute('aria-label', text);
    btn.setAttribute('data-lc-tip', text);
  }

  // ---- LC-854 transport + input replay ------------------------------
  // The sharer's tile <video> the controller drives (the shared surface). The
  // roster keys tiles by user id, exactly as voice.js builds them.
  function tileVideo(uid) {
    if (!uid) return null;
    var esc = (window.CSS && CSS.escape) ? CSS.escape(uid) : uid.replace(/"/g, '\\"');
    return document.querySelector('[data-lc-voice-tile="' + esc + '"] [data-lc-voice-video]');
  }
  var sfu = function () { return window.LetsChatHuddleSfu; };
  // Controller: start capturing over the sharer's tile and shipping frames.
  // Movement is unreliable (drop-old), clicks/keys reliable, matching the 1:1
  // channel. Coordinates are normalized [0,1] over the surface; the injector
  // maps to the sharer's virtual desktop. Known limitation (carried from the
  // 1:1 flow, inject.rs): a multi-monitor share maps to the whole virtual
  // desktop, so per-monitor coordinates are off - single/primary shares are
  // correct.
  function startCapture(sharerId) {
    if (captureStop || !window.LetsChatRtc || !window.LetsChatRtc.control) return;
    var s = sfu();
    if (!s || !s.sendControl) return; // mesh huddle: no data channel (unavailable)
    controllingSharer = sharerId;
    // Learn the sharer's injector capability from their cap frame (LC-640):
    // this is our only inbound data as the controller.
    if (s.onControlData) {
      s.onControlData(function (fromId, text) {
        if (fromId !== sharerId) return;
        var cap = parseCap(text);
        if (cap) { sharerNative = !!cap.native; updateNoInject(); }
      });
    }
    captureStop = window.LetsChatRtc.control.bindCapture(
      function () { return tileVideo(sharerId); },
      function (frame) {
        var reliable = frame.t !== 'm';
        s.sendControl(JSON.stringify(frame), sharerId, reliable);
      }
    );
  }
  function stopCapture() {
    if (captureStop) { try { captureStop(); } catch (e) {} captureStop = null; }
    var s = sfu();
    if (s && s.onControlData) s.onControlData(null);
    controllingSharer = null;
    sharerNative = null;
    updateNoInject();
  }
  // Sharer: receive the controller's frames and hand each to the native
  // injector (LC-185) via the same DOM event the 1:1 flow uses; a web sharer
  // has no injector and the event is unobserved. Only frames from the granted
  // controller are honored.
  function armInjection() {
    var s = sfu();
    if (!s || !s.onControlData) return;
    s.onControlData(function (fromId, text) {
      if (!grantedTo || fromId !== grantedTo) return;
      var cap = parseCap(text);
      if (cap) return; // capability frames are controller-bound, ignore here
      try {
        document.dispatchEvent(new CustomEvent('lc:control-input', { detail: text }));
      } catch (e) {}
    });
    // Announce our injector capability so the controller learns whether their
    // input will land, without any server involvement (LC-640). Sent now and
    // once more shortly after: the grant rides the WS while this rides the
    // LiveKit data channel, so the controller may not have armed its receiver
    // by the first send. Both are idempotent.
    var to = grantedTo;
    var cap = JSON.stringify({ t: 'cap', native: !!window.__lcNativeControl });
    s.sendControl(cap, to, true);
    setTimeout(function () { if (grantedTo === to) s.sendControl(cap, to, true); }, 500);
  }
  function disarmInjection() {
    var s = sfu();
    if (s && s.onControlData) s.onControlData(null);
    // Release any keys/buttons the injector is holding.
    try { document.dispatchEvent(new CustomEvent('lc:control-end')); } catch (e) {}
  }
  function parseCap(text) {
    if (typeof text !== 'string') return null;
    try { var o = JSON.parse(text); return (o && o.t === 'cap') ? o : null; } catch (e) { return null; }
  }
  function updateNoInject() {
    var note = q('[data-lc-huddle-control-noinject]');
    if (!note) return;
    var show = phase === 'controlling' && sharerNative === false;
    note.classList.toggle('hidden', !show);
    note.setAttribute('aria-hidden', show ? 'false' : 'true');
  }

  // Requester side ----------------------------------------------------
  function endControlling(msg) {
    if (requestTimer) { clearTimeout(requestTimer); requestTimer = null; }
    stopCapture();
    phase = 'idle';
    setRequestBtn();
    if (msg) announce(msg);
  }
  function onRequestClick() {
    if (phase === 'controlling') {
      send('revoke');
      endControlling(str('huddleControlEnded', 'Control ended'));
      return;
    }
    if (phase === 'requesting' || !joined()) return;
    phase = 'requesting';
    setRequestBtn();
    announce(str('callRequesting', 'Requesting...'));
    send('request');
    requestTimer = setTimeout(function () {
      if (phase === 'requesting') {
        endControlling(str('callControlNoAnswer', 'Control request not answered'));
      }
    }, REQUEST_TIMEOUT_MS);
  }

  // Sharer side -------------------------------------------------------
  function showPrompt(fromId, fromName) {
    pendingFrom = fromId;
    pendingFromName = fromName;
    var nm = q('[data-lc-huddle-control-prompt-name]');
    if (nm) nm.textContent = fromName;
    show(q('[data-lc-huddle-control-prompt]'));
    announce(fromName + ' ' + str('huddleControlRequested', 'requested control of your screen'));
    if (promptTimer) clearTimeout(promptTimer);
    promptTimer = setTimeout(denyPending, PROMPT_AUTODENY_MS);
  }
  function hidePrompt() {
    if (promptTimer) { clearTimeout(promptTimer); promptTimer = null; }
    pendingFrom = null;
    pendingFromName = '';
    hide(q('[data-lc-huddle-control-prompt]'));
  }
  function denyPending() {
    if (pendingFrom == null) return;
    send('deny');
    hidePrompt();
  }
  function grantPending() {
    if (pendingFrom == null) return;
    grantedTo = pendingFrom;
    var nm = q('[data-lc-huddle-control-banner-name]');
    if (nm) nm.textContent = pendingFromName;
    announce(pendingFromName + ' ' + str('huddleControlActiveSuffix', 'is controlling your screen'));
    hidePrompt();
    send('grant');
    // LC-854: begin accepting the controller's input frames and hand them to
    // the native injector. Must arm before the grant signal could race a first
    // frame back, so onControlData is registered first.
    armInjection();
    show(q('[data-lc-huddle-control-banner]'));
  }
  function endGrant(msg) {
    if (!grantedTo) return;
    grantedTo = null;
    disarmInjection();
    hide(q('[data-lc-huddle-control-banner]'));
    if (msg) announce(msg);
  }

  // Inbound consent signals ------------------------------------------
  function onControlEvent(node) {
    var msgRoomId = parseInt(node.getAttribute('data-room-id'), 10);
    if (isNaN(msgRoomId) || msgRoomId !== roomId()) return;
    var kind = node.getAttribute('data-kind');
    var fromId = node.getAttribute('data-from-id');
    var fromName = node.getAttribute('data-from-name') || str('callAContact', 'A contact');
    switch (kind) {
      case 'request':
        // Only the sharer is ever asked; the server routes to the sharer, but
        // be defensive about a stale frame after our share ended.
        if (!sharers[selfId()]) return;
        showPrompt(fromId, fromName);
        break;
      case 'grant':
        if (phase !== 'requesting') return;
        if (requestTimer) { clearTimeout(requestTimer); requestTimer = null; }
        phase = 'controlling';
        // LC-854: the grant names the sharer (from_user_id); begin capturing
        // over their tile and shipping input over the data channel.
        startCapture(fromId);
        setRequestBtn();
        updateNoInject();
        announce(str('huddleControlGranted', 'Control granted'));
        break;
      case 'deny':
        if (phase === 'requesting') endControlling(str('callControlDenied', 'Control request denied'));
        break;
      case 'busy':
        if (phase === 'requesting') endControlling(str('huddleControlBusy', 'Someone else already has control'));
        break;
      case 'unavailable':
        if (phase === 'requesting') endControlling(str('callControlUnavailable', 'Remote control is not available in this call'));
        break;
      case 'revoke':
        // Either role: the sharer ended our control, or our controller (or
        // the server's auto-revoke) ended the grant we gave.
        hidePrompt();
        if (phase !== 'idle') endControlling(str('huddleControlEnded', 'Control ended'));
        endGrant(str('huddleControlEnded', 'Control ended'));
        break;
    }
  }

  // Voice-channel presence: track who shares, react to shares/people ending.
  function onVoiceEvent(node) {
    var msgRoomId = parseInt(node.getAttribute('data-room-id'), 10);
    if (isNaN(msgRoomId) || msgRoomId !== roomId()) return;
    var kind = node.getAttribute('data-kind');
    var uid = node.getAttribute('data-user-id');
    if (kind === 'screen') {
      var on = node.getAttribute('data-payload') === '1';
      if (uid) sharers[uid] = on;
      if (!on) {
        if (uid === selfId()) {
          // Our share ended: the server auto-revokes the session and the
          // pending slot; drop the local sharer UI to match.
          hidePrompt();
          endGrant(str('huddleControlEnded', 'Control ended'));
        } else if (phase === 'requesting' && !canRequest({ enabled: true, joined: true, selfId: selfId(), sharers: sharers })) {
          // The share we asked about vanished before an answer.
          endControlling(str('callControlUnavailable', 'Remote control is not available in this call'));
        }
      }
      setRequestBtn();
    } else if (kind === 'left') {
      if (uid) delete sharers[uid];
      if (uid === selfId()) {
        // We left the huddle: every control role dies with the call.
        sharers = {};
        hidePrompt();
        endGrant();
        endControlling();
        announce('');
      } else if (uid === grantedTo) {
        endGrant(str('huddleControlEnded', 'Control ended'));
      }
      setRequestBtn();
    } else if (kind === 'joined') {
      setRequestBtn();
    }
  }

  // Install unconditionally (the buses live in layout.html on every page):
  // an htmx page swap can bring the huddle bar in AFTER load, so presence of
  // the bar is checked live in every handler via root(), never at init.
  if (!window.LetsChatRtc) return;
  window.LetsChatRtc.watchBus('lc-control-bus', 'data-lc-control-event', onControlEvent);
  window.LetsChatRtc.watchBus('lc-voice-bus', 'data-lc-voice-event', onVoiceEvent);
  window.LetsChatRtc.bindControls({
    '[data-lc-huddle-control-request]': onRequestClick,
    '[data-lc-huddle-control-grant]': grantPending,
    '[data-lc-huddle-control-deny]': denyPending,
    '[data-lc-huddle-control-stop]': function () {
      send('revoke');
      endGrant(str('huddleControlEnded', 'Control ended'));
    },
  });
})();
