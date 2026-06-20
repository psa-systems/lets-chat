// LC-393: call transcription (1:1 DM calls + enclave voice channels).
//
// Call media is peer-to-peer - the server never sees the audio - so each
// participant's browser captures its OWN microphone. There are two engines,
// chosen by server config (/call/config -> sttServer):
//  - browser (default): the Web Speech API (SpeechRecognition) transcribes
//    locally and POSTs final text.
//  - server (Phase 3): MediaRecorder captures short audio clips and POSTs them
//    to /audio; the server forwards each to the operator's STT endpoint. This
//    is browser-agnostic (Firefox/Safari) and keeps audio off third-party clouds.
// Either way speaker attribution is automatic: a segment's speaker is whoever's
// browser produced it.
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
  var MR = window.MediaRecorder;

  function q(sel) { return document.querySelector(sel); }

  var transcriptId = null; // active session id, set from the TranscriptStarted bus event
  var recog = null;        // SpeechRecognition instance while capturing (browser engine)
  var capturing = false;

  // LC-393 Phase 3: when the operator has configured a server-side STT endpoint,
  // we capture short audio clips with MediaRecorder and POST them instead of
  // using the in-browser Web Speech API (browser-agnostic + keeps audio local).
  // Learn which engine to use from /call/config (server-wide config; same for
  // every participant). Until it resolves we assume the browser engine.
  var sttServer = false;
  var CLIP_MS = 5000;      // length of each server-STT audio clip
  var sttStream = null;    // mic stream for MediaRecorder
  var sttRecorder = null;
  var sttTimer = null;
  function loadConfig() {
    fetch('/call/config', { credentials: 'same-origin' })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (j) {
        if (j && typeof j.sttServer === 'boolean') sttServer = j.sttServer;
        disableIfUnsupported();
      })
      .catch(function () {});
  }

  // ---- UI state ---------------------------------------------------------
  // LC-402: "active" UI = the recording pill (awareness) + the transcript
  // drawer. They were one toggle before; split so the drawer can be closed /
  // reopened independently while a session stays live.
  function showBanner(on) {
    var b = q('[data-lc-transcript-banner]');
    if (b) {
      b.classList.toggle('hidden', !on);
      b.setAttribute('aria-hidden', on ? 'false' : 'true');
    }
    showPanel(on);
  }
  function showPanel(on) {
    var w = q('[data-lc-caption-wrap]');
    if (w) {
      w.classList.toggle('hidden', !on);
      w.setAttribute('aria-hidden', on ? 'false' : 'true');
    }
    document.body.classList.toggle('lc-transcript-open', !!on);
    var pt = q('[data-lc-transcript-panel-toggle]');
    if (pt) pt.setAttribute('aria-pressed', on ? 'true' : 'false');
    if (on) scrollToLive();
  }
  function setToggle(on) {
    var t = q('[data-lc-transcribe-toggle]');
    if (t) t.setAttribute('aria-pressed', on ? 'true' : 'false');
  }

  // ---- caption scroll / jump-to-live (LC-402) ---------------------------
  function transcriptBody() { return q('[data-lc-transcript-body]'); }
  function nearBottom(el) { return el.scrollHeight - el.scrollTop - el.clientHeight < 48; }
  function scrollToLive() {
    var el = transcriptBody();
    if (el) el.scrollTop = el.scrollHeight;
    document.body.classList.remove('lc-transcript-scrolled');
  }
  function onNewCaptions() {
    var el = transcriptBody();
    if (!el) return;
    if (nearBottom(el)) scrollToLive();
    else document.body.classList.add('lc-transcript-scrolled');
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
  // Pick the engine per the server config; each captures the user's own mic.
  function startLocalCapture() {
    if (capturing) return;
    if (sttServer) startServerCapture(); else startBrowserCapture();
  }
  function stopLocalCapture() {
    stopBrowserCapture();
    stopServerCapture();
    capturing = false;
  }

  // Browser engine: Web Speech API, final results posted as text (Phase 1/2).
  function startBrowserCapture() {
    if (!SR) return;
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
  function stopBrowserCapture() {
    if (recog) {
      try { recog.onend = null; recog.stop(); } catch (e) {}
      recog = null;
    }
  }

  // Server engine: record fixed-length mic clips with MediaRecorder and POST
  // each to /audio for server-side STT. Each clip is a complete, independently
  // decodable file (start->stop per clip), which the engine can transcribe;
  // recording the next clip starts immediately so gaps at the boundary are tiny.
  function pickMime() {
    var types = ['audio/webm;codecs=opus', 'audio/webm', 'audio/ogg;codecs=opus', 'audio/mp4'];
    for (var i = 0; i < types.length; i++) {
      try { if (MR.isTypeSupported(types[i])) return types[i]; } catch (e) {}
    }
    return '';
  }
  function startServerCapture() {
    if (!MR) return;
    capturing = true;
    navigator.mediaDevices.getUserMedia({ audio: true }).then(function (stream) {
      if (!capturing) { stream.getTracks().forEach(function (t) { try { t.stop(); } catch (e) {} }); return; }
      sttStream = stream;
      recordClip();
    }).catch(function () { capturing = false; });
  }
  function recordClip() {
    if (!capturing || !sttStream) return;
    var mime = pickMime();
    try { sttRecorder = mime ? new MR(sttStream, { mimeType: mime }) : new MR(sttStream); }
    catch (e) { capturing = false; return; }
    var chunks = [];
    sttRecorder.ondataavailable = function (e) { if (e.data && e.data.size) chunks.push(e.data); };
    sttRecorder.onstop = function () {
      var type = (sttRecorder && sttRecorder.mimeType) || mime || 'audio/webm';
      var blob = new Blob(chunks, { type: type });
      if (capturing) recordClip();       // begin the next clip right away
      if (blob.size > 0) postAudio(blob); // ...and ship this one
    };
    try { sttRecorder.start(); } catch (e) { capturing = false; return; }
    sttTimer = setTimeout(function () { try { sttRecorder.stop(); } catch (e) {} }, CLIP_MS);
  }
  function postAudio(blob) {
    if (transcriptId == null || !blob.size) return;
    fetch('/call/transcript/' + transcriptId + '/audio', {
      method: 'POST',
      headers: { 'Content-Type': blob.type || 'audio/webm' },
      body: blob,
      credentials: 'same-origin'
    }).catch(function () {});
  }
  function stopServerCapture() {
    if (sttTimer) { clearTimeout(sttTimer); sttTimer = null; }
    if (sttRecorder) { try { sttRecorder.onstop = null; sttRecorder.stop(); } catch (e) {} sttRecorder = null; }
    if (sttStream) { sttStream.getTracks().forEach(function (t) { try { t.stop(); } catch (e) {} }); sttStream = null; }
  }

  // ---- session control --------------------------------------------------
  // The room comes from the clicked toggle's data-lc-room (voice channel, where
  // the room is static + server-rendered), else the active 1:1 call / voice room
  // published by call.js / voice.js.
  function startSession(toggleEl) {
    var room = (toggleEl && toggleEl.getAttribute('data-lc-room')) ||
      window.__lcCallRoom || window.__lcVoiceRoom;
    if (room == null) return;
    // The browser engine needs SpeechRecognition; the server engine needs
    // MediaRecorder. Block only when the engine we'd actually use is missing.
    var ok = sttServer ? !!MR : !!SR;
    if (!ok) {
      var msg = (toggleEl && toggleEl.getAttribute('data-lc-unsupported')) ||
        'Live transcription is not supported in this browser.';
      alert(msg);
      return;
    }
    // LC-396: drive OUR OWN UI directly from the response (it returns
    // {transcript_id}) - do NOT depend on receiving our own TranscriptStarted
    // event back over the WebSocket. That self-echo was an extra, fragile hop
    // and when it didn't arrive the starter saw nothing at all (notably on 1:1
    // calls). The #lc-transcript-bus event still drives the OTHER participants;
    // the two paths are idempotent (capture guards on `capturing`, the banner
    // toggle is a no-op when already shown). Failures are surfaced, not swallowed.
    fetch('/call/' + room + '/transcript/start', {
      method: 'POST',
      credentials: 'same-origin'
    }).then(function (r) {
      if (!r.ok) {
        try { console.warn('lets-chat: transcription start failed', r.status); } catch (e) {}
        alert('Could not start transcription (error ' + r.status + ').');
        return null;
      }
      return r.json();
    }).then(function (j) {
      if (j && j.transcript_id != null) {
        transcriptId = parseInt(j.transcript_id, 10);
        showBanner(true);
        setToggle(true);
        startLocalCapture();
      }
    }).catch(function () {
      alert('Could not start transcription (network error).');
    });
  }
  function endSession() {
    var id = transcriptId;
    transcriptId = null;
    stopLocalCapture();
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
        startLocalCapture();
      } else if (kind === 'ended') {
        transcriptId = null;
        stopLocalCapture();
        showBanner(false);
        setToggle(false);
      }
    }
  }
  var bus = q('#lc-transcript-bus');
  if (bus && window.MutationObserver) {
    new MutationObserver(drainBus).observe(bus, { childList: true });
  }
  // Belt-and-braces: also drain after each WS frame settles. Caption
  // grouping/timestamping + jump-to-live are handled by the observer below.
  document.body.addEventListener('htmx:wsAfterMessage', drainBus);

  // ---- caption decoration: timestamp + speaker grouping (LC-402) --------
  // Each line arrives as server-rendered HTML (ws/transcript_segment.html) with
  // an empty .lc-cap-time and a data-lc-speaker. Stamp the local receipt time
  // and collapse consecutive lines from one speaker (CSS hides the repeat head).
  function decorateLine(line) {
    if (!line || line.nodeType !== 1 || !line.classList.contains('lc-cap-line')) return;
    if (line.getAttribute('data-lc-decorated')) return;
    line.setAttribute('data-lc-decorated', '1');
    var t = line.querySelector('.lc-cap-time');
    if (t && !t.textContent) {
      try {
        t.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      } catch (e) {}
    }
    var prev = line.previousElementSibling;
    if (prev && prev.classList.contains('lc-cap-line') &&
        prev.getAttribute('data-lc-speaker') === line.getAttribute('data-lc-speaker')) {
      line.classList.add('lc-cap-cont');
    }
  }
  var log = document.getElementById('lc-caption-log');
  if (log && window.MutationObserver) {
    // Decorate anything already present, then watch for appended lines.
    Array.prototype.forEach.call(log.querySelectorAll('.lc-cap-line'), decorateLine);
    new MutationObserver(function (muts) {
      for (var i = 0; i < muts.length; i++) {
        Array.prototype.forEach.call(muts[i].addedNodes, decorateLine);
      }
      onNewCaptions();
    }).observe(log, { childList: true });
  }
  // Track scroll position so the jump-to-live affordance reveals when scrolled up.
  document.addEventListener('scroll', function (e) {
    var el = transcriptBody();
    if (!el || e.target !== el) return;
    document.body.classList.toggle('lc-transcript-scrolled', !nearBottom(el));
  }, true);

  // ---- transcript drawer controls (LC-402) ------------------------------
  document.addEventListener('click', function (e) {
    if (!e.target.closest) return;
    if (e.target.closest('[data-lc-transcript-close]')) { showPanel(false); return; }
    if (e.target.closest('[data-lc-jump-live]')) { scrollToLive(); return; }
    var pt = e.target.closest('[data-lc-transcript-panel-toggle]');
    if (pt) {
      var w = q('[data-lc-caption-wrap]');
      showPanel(!w || w.classList.contains('hidden'));
    }
  });

  // ---- call lifecycle ---------------------------------------------------
  // Disable every transcription toggle when the engine we'd use is unavailable:
  // the server engine needs MediaRecorder, the browser engine SpeechRecognition.
  function disableIfUnsupported() {
    var supported = sttServer ? !!MR : !!SR;
    var toggles = document.querySelectorAll('[data-lc-transcribe-toggle]');
    for (var i = 0; i < toggles.length; i++) {
      toggles[i].disabled = !supported;
      if (!supported) toggles[i].title = toggles[i].getAttribute('data-lc-unsupported') || '';
    }
  }

  // 1:1 call ended (call.js): finalize the session for everyone.
  document.addEventListener('lc:call-ended', function () {
    if (transcriptId != null) endSession();
    else { stopLocalCapture(); showBanner(false); setToggle(false); }
  });
  // Left a voice channel (voice.js): stop OUR capture + reset local UI, but do
  // NOT /end - the shared session continues for whoever is still in the channel.
  document.addEventListener('lc:voice-left', function () {
    transcriptId = null;
    stopLocalCapture();
    showBanner(false);
    setToggle(false);
  });
  document.addEventListener('lc:call-active', disableIfUnsupported);
  document.addEventListener('lc:voice-joined', disableIfUnsupported);

  // Learn the engine (browser vs server STT) up front, then keep toggles in step.
  loadConfig();
})();
