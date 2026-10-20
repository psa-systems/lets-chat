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
  // One short sine tone: freq Hz, starting `offset` s from now, lasting `dur`
  // s, ramped up to `peak` and back down so it never clicks.
  function beep(freq, offset, dur, peak) {
    if (!ctx) return;
    var t = ctx.currentTime + offset;
    var osc = ctx.createOscillator();
    var g = ctx.createGain();
    osc.type = 'sine';
    osc.frequency.value = freq;
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(peak || 0.12, t + 0.015);
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    osc.connect(g);
    g.connect(ctx.destination);
    osc.start(t);
    osc.stop(t + dur + 0.02);
  }

  // Someone else joined (rising two-note) / left (falling two-note) the room.
  function playJoin() { if (soundEnabled() && ac()) { beep(523, 0, 0.18, 0.1); beep(784, 0.12, 0.22, 0.1); } }
  function playLeave() { if (soundEnabled() && ac()) { beep(587, 0, 0.18, 0.1); beep(392, 0.12, 0.26, 0.1); } }

  // Incoming-call ringtone: a classic double-ring that repeats until stopRing.
  var ringTimer = null;
  function ringOnce() { if (ctx) { beep(480, 0, 0.4, 0.16); beep(620, 0.45, 0.4, 0.16); } }
  function startRing() {
    if (ringTimer || !soundEnabled() || !ac()) return;
    ringOnce();
    ringTimer = setInterval(function () { if (ac()) ringOnce(); }, 2600);
  }
  function stopRing() { if (ringTimer) { clearInterval(ringTimer); ringTimer = null; } }

  window.LetsChatSfx = {
    formatDuration: formatDuration,
    soundEnabled: soundEnabled,
    playJoin: playJoin,
    playLeave: playLeave,
    startRing: startRing,
    stopRing: stopRing,
  };
})();
