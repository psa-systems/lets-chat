// LC-393: call transcription (Phase 1, 1:1 DM calls).
//
// Call media is peer-to-peer - the server never sees the audio - so each
// participant's browser transcribes its OWN microphone with the Web Speech API
// (SpeechRecognition) and POSTs final results to the server, which stores them
// and fans out attributed live captions to both parties. Speaker attribution is
// therefore automatic: a segment's speaker is whoever's browser produced it.
//
// Lifecycle: the toggle button POSTs /start; the server opens a session and
// broadcasts TranscriptStarted to BOTH members (consent banner + each client
// auto-starts its own capture). On hangup (or toggle off) /end finalizes it and
// the server posts a linked "transcript saved" notice into the DM. The display
// of caption lines is pure server-rendered OOB swaps into #lc-caption-log; this
// module only handles capture, the POSTs, and the banner/toggle state.
(function () {
  'use strict';

  var SR = window.SpeechRecognition || window.webkitSpeechRecognition;

  function q(sel) { return document.querySelector(sel); }

  var transcriptId = null; // active session id, set from the TranscriptStarted bus event
  var recog = null;        // SpeechRecognition instance while capturing
  var capturing = false;

  // ---- UI state ---------------------------------------------------------
  function showBanner(on) {
    var b = q('[data-lc-transcript-banner]');
    var w = q('[data-lc-caption-wrap]');
    [b, w].forEach(function (el) {
      if (!el) return;
      el.classList.toggle('hidden', !on);
      el.setAttribute('aria-hidden', on ? 'false' : 'true');
    });
  }
  function setToggle(on) {
    var t = q('[data-lc-transcribe-toggle]');
    if (t) t.setAttribute('aria-pressed', on ? 'true' : 'false');
  }

  // ---- segment POST -----------------------------------------------------
  function postSegment(text) {
    if (transcriptId == null || !text) return;
    var body = new URLSearchParams();
    body.set('text', text);
    fetch('/call/transcript/' + transcriptId + '/segment', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: body.toString(),
      credentials: 'same-origin'
    }).catch(function () {});
  }

  // ---- local mic capture ------------------------------------------------
  function startCapture() {
    if (!SR || capturing) return;
    try { recog = new SR(); } catch (e) { return; }
    recog.continuous = true;
    recog.interimResults = false;
    recog.lang = navigator.language || 'en-US';
    recog.onresult = function (ev) {
      for (var i = ev.resultIndex; i < ev.results.length; i++) {
        var r = ev.results[i];
        if (r.isFinal) {
          var t = (r[0] && r[0].transcript || '').trim();
          if (t) postSegment(t);
        }
      }
    };
    // 'no-speech' / 'aborted' fire routinely; onend below restarts so a long
    // silence doesn't permanently stop capture.
    recog.onerror = function () {};
    recog.onend = function () {
      if (capturing) { try { recog.start(); } catch (e) {} }
    };
    capturing = true;
    try { recog.start(); } catch (e) { capturing = false; }
  }
  function stopCapture() {
    capturing = false;
    if (recog) {
      try { recog.onend = null; recog.stop(); } catch (e) {}
      recog = null;
    }
  }

  // ---- session control --------------------------------------------------
  // The room comes from the clicked toggle's data-lc-room (voice channel, where
  // the room is static + server-rendered), else the active 1:1 call / voice room
  // published by call.js / voice.js.
  function startSession(toggleEl) {
    var room = (toggleEl && toggleEl.getAttribute('data-lc-room')) ||
      window.__lcCallRoom || window.__lcVoiceRoom;
    if (room == null) return;
    if (!SR) {
      var msg = (toggleEl && toggleEl.getAttribute('data-lc-unsupported')) ||
        'Live transcription is not supported in this browser.';
      alert(msg);
      return;
    }
    // The TranscriptStarted bus event (to us, too) drives the banner + capture,
    // so this POST only needs to open the session server-side.
    fetch('/call/' + room + '/transcript/start', {
      method: 'POST',
      credentials: 'same-origin'
    }).catch(function () {});
  }
  function endSession() {
    var id = transcriptId;
    transcriptId = null;
    stopCapture();
    showBanner(false);
    setToggle(false);
    if (id != null) {
      fetch('/call/transcript/' + id + '/end', {
        method: 'POST',
        credentials: 'same-origin'
      }).catch(function () {});
    }
  }

  // ---- toggle button ----------------------------------------------------
  document.addEventListener('click', function (e) {
    var t = e.target.closest && e.target.closest('[data-lc-transcribe-toggle]');
    if (!t) return;
    if (transcriptId == null) startSession(t); else endSession();
  });

  // ---- transcript control bus (started / ended) -------------------------
  function drainBus() {
    var bus = q('#lc-transcript-bus');
    if (!bus) return;
    var nodes = bus.querySelectorAll('[data-lc-transcript-event]');
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      var kind = n.getAttribute('data-kind');
      var id = n.getAttribute('data-transcript-id');
      n.remove();
      if (kind === 'started') {
        transcriptId = id ? parseInt(id, 10) : null;
        showBanner(true);
        setToggle(true);
        startCapture();
      } else if (kind === 'ended') {
        transcriptId = null;
        stopCapture();
        showBanner(false);
        setToggle(false);
      }
    }
  }
  var bus = q('#lc-transcript-bus');
  if (bus && window.MutationObserver) {
    new MutationObserver(drainBus).observe(bus, { childList: true });
  }
  // Belt-and-braces: also drain + autoscroll after each WS frame settles.
  document.body.addEventListener('htmx:wsAfterMessage', function () {
    drainBus();
    var w = q('[data-lc-caption-wrap]');
    if (w) w.scrollTop = w.scrollHeight;
  });

  // ---- call lifecycle ---------------------------------------------------
  // Disable every transcription toggle when the browser has no SpeechRecognition.
  function disableIfUnsupported() {
    if (SR) return;
    var toggles = document.querySelectorAll('[data-lc-transcribe-toggle]');
    for (var i = 0; i < toggles.length; i++) {
      toggles[i].disabled = true;
      toggles[i].title = toggles[i].getAttribute('data-lc-unsupported') || '';
    }
  }

  // 1:1 call ended (call.js): finalize the session for everyone.
  document.addEventListener('lc:call-ended', function () {
    if (transcriptId != null) endSession();
    else { stopCapture(); showBanner(false); setToggle(false); }
  });
  // Left a voice channel (voice.js): stop OUR capture + reset local UI, but do
  // NOT /end - the shared session continues for whoever is still in the channel.
  document.addEventListener('lc:voice-left', function () {
    transcriptId = null;
    stopCapture();
    showBanner(false);
    setToggle(false);
  });
  document.addEventListener('lc:call-active', disableIfUnsupported);
  document.addEventListener('lc:voice-joined', disableIfUnsupported);
})();
