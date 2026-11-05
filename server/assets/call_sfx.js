// LC-1063: small shared sound-and-duration helper for calls.
//
// Two unrelated call surfaces want a few audio cues: the 1-1 call (call.js)
// needs an incoming-call ringtone and a call-duration readout, and a voice
// room (voice.js) wants a short join / leave cue. Rather than ship binary
// audio assets (the repo has exactly one, notify.ogg) and duplicate the
// WebAudio plumbing in both files, synthesize the cues here with short
// oscillator tones and expose one tiny API.
//
// Every cue is gated by the SAME preference layout.html reads for notify.ogg:
// data-sound-enabled on #lc-mention-counts (user.notify_sound_enabled). A
// browser with no Web Audio, or a suspended context behind an autoplay policy,
// degrades silently - a missing beep never breaks a call.
//
// The pure helper (formatDuration) is exported via module.exports for
// call_sfx.test.js; the audio surface is a no-op under Node.
(function () {
  'use strict';

  // ---- pure helper (unit-tested) ---------------------------------------
  // Seconds -> "m:ss", or "h:mm:ss" once the call passes an hour. Never
  // negative; fractional input floors.
  function formatDuration(totalSecs) {
    totalSecs = Math.max(0, Math.floor(Number(totalSecs) || 0));
    var h = Math.floor(totalSecs / 3600);
    var m = Math.floor((totalSecs % 3600) / 60);
    var s = totalSecs % 60;
    function p(n) { return (n < 10 ? '0' : '') + n; }
    return h > 0 ? h + ':' + p(m) + ':' + p(s) : m + ':' + p(s);
  }

  var api = { formatDuration: formatDuration };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window === 'undefined' || typeof document === 'undefined') return;

  // ---- audio ------------------------------------------------------------
  function soundEnabled() {
    var cfg = document.getElementById('lc-mention-counts');
    return !!cfg && cfg.getAttribute('data-sound-enabled') === '1';
  }

  var AC = window.AudioContext || window.webkitAudioContext;
  var ctx = null;
  // Lazily create (and nudge out of the autoplay-suspended state) one shared
  // context. Returns null when Web Audio is unavailable.
  function ac() {
    if (!AC) return null;
    if (!ctx) { try { ctx = new AC(); } catch (e) { ctx = null; return null; } }
    if (ctx.state === 'suspended') { try { ctx.resume(); } catch (e) {} }
    return ctx;
  }
  // One short tone: freq Hz, starting `offset` s from now, lasting `dur` s,
  // ramped up to `peak` and back down so it never clicks. `type` picks the
  // oscillator waveform ('sine' by default; 'triangle' is brighter and carries
  // better over call audio, so the cheerful cues use it).
  function beep(freq, offset, dur, peak, type) {
    if (!ctx) return;
    var t = ctx.currentTime + offset;
    var osc = ctx.createOscillator();
    var g = ctx.createGain();
    osc.type = type || 'sine';
    osc.frequency.value = freq;
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(peak || 0.12, t + 0.015);
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    osc.connect(g);
    g.connect(ctx.destination);
    osc.start(t);
    osc.stop(t + dur + 0.02);
  }

  // LC-1120: someone else joined - a bright, cheerful rising major triad
  // (C5-E5-G5) on a triangle wave, louder than the old two-note cue so it is
  // easy to notice over call audio. Leaving is a softer falling two-note so it
  // reads as quieter/less celebratory but is still audible.
  function playJoin() {
    if (soundEnabled() && ac()) {
      beep(523.25, 0, 0.16, 0.26, 'triangle');
      beep(659.25, 0.1, 0.16, 0.28, 'triangle');
      beep(783.99, 0.2, 0.32, 0.34, 'triangle');
    }
  }
  function playLeave() {
    if (soundEnabled() && ac()) {
      beep(659.25, 0, 0.16, 0.2, 'triangle');
      beep(440.0, 0.12, 0.28, 0.2, 'triangle');
    }
  }

  // Incoming-call ringtone: a classic double-ring that repeats until stopRing.
  // Louder than the cues so an incoming call is unmistakable.
  var ringTimer = null;
  function ringOnce() { if (ctx) { beep(523.25, 0, 0.4, 0.24, 'triangle'); beep(659.25, 0.45, 0.4, 0.24, 'triangle'); } }
  function startRing() {
    if (ringTimer || !soundEnabled() || !ac()) return;
    ringOnce();
    ringTimer = setInterval(function () { if (ac()) ringOnce(); }, 2600);
  }
  function stopRing() { if (ringTimer) { clearInterval(ringTimer); ringTimer = null; } }

  // LC-1120: prime the shared context on the first real user gesture. Every cue
  // above (incoming-call ring, a peer join/leave) fires from a WS event with no
  // user gesture in the call stack, so a context first created there starts
  // 'suspended' and resume() loses the race against the immediate beep
  // scheduling - the tones are scheduled on a not-yet-running context and never
  // sound. Creating + resuming it here, inside a genuine gesture, and nudging it
  // with a zero-gain tick leaves it 'running' well before any cue, so the ring
  // and join cue are audible. One-shot: the listeners remove themselves on the
  // first gesture whether or not sound is enabled.
  function primeAudio() {
    removePrime();
    if (!soundEnabled()) return;
    var ctx = ac();
    if (!ctx) return;
    try {
      var o = ctx.createOscillator();
      var g = ctx.createGain();
      g.gain.value = 0;
      o.connect(g);
      g.connect(ctx.destination);
      o.start();
      o.stop(ctx.currentTime + 0.01);
    } catch (e) {}
  }
  function removePrime() {
    ['pointerdown', 'keydown', 'touchstart'].forEach(function (ev) {
      document.removeEventListener(ev, primeAudio, true);
    });
  }
  ['pointerdown', 'keydown', 'touchstart'].forEach(function (ev) {
    document.addEventListener(ev, primeAudio, true);
  });

  window.LetsChatSfx = {
    formatDuration: formatDuration,
    soundEnabled: soundEnabled,
    playJoin: playJoin,
    playLeave: playLeave,
    startRing: startRing,
    stopRing: stopRing,
  };
})();
