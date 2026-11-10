// LC-843: run with `just test-js` (node --test). transcribe.js is a browser
// IIFE; it is evaluated in a VM sandbox with just enough DOM/fetch stubbed for
// its load-time wiring, then the exported pure engine decision is pinned. The
// matrix is the contract: the Accurate preference only wins when the server
// engine is really available, the Fast preference only wins when SR exists, a
// dead browser engine (serverFallback) beats any preference, and clients that
// never touch the control keep the pre-LC-843 automatic behavior.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function load() {
  const src = fs.readFileSync(path.join(__dirname, 'transcribe.js'), 'utf8');
  const stored = {};
  const els = { body: { addEventListener() {}, classList: { toggle() {}, add() {}, remove() {} } } };
  const window = {};
  const document = {
    body: els.body,
    addEventListener() {},
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementById: () => null,
  };
  const sandbox = {
    window,
    document,
    localStorage: {
      getItem: (k) => (k in stored ? stored[k] : null),
      setItem: (k, v) => { stored[k] = String(v); },
    },
    fetch: () => Promise.resolve({ ok: false }),
    navigator: {},
    console,
    setTimeout,
    clearTimeout,
  };
  vm.runInNewContext(src, sandbox);
  return { window, stored };
}

test('the engine decision matrix', () => {
  const choose = load().window.LetsChatTranscribe.chooseEngine;
  // (pref, hasSR, hasMR, sttServer, serverFallback)
  // Automatic (no preference): the pre-LC-843 behavior, unchanged.
  assert.equal(choose(null, true, true, true, false), 'browser', 'auto prefers the fast path');
  assert.equal(choose(null, true, true, false, false), 'browser');
  assert.equal(choose(null, false, true, true, false), 'server', 'no SR + operator STT = server');
  assert.equal(choose(null, false, true, false, false), 'browser', 'nothing available degrades like before');
  // Accurate preference: honored only when the server engine really exists.
  assert.equal(choose('accurate', true, true, true, false), 'server', 'the LC-843 switch');
  assert.equal(choose('accurate', true, true, false, false), 'browser', 'no operator STT: pref cannot conjure it');
  assert.equal(choose('accurate', true, false, true, false), 'browser', 'no MediaRecorder: pref cannot conjure it');
  // Fast preference: honored only when SR exists.
  assert.equal(choose('fast', true, true, true, false), 'browser');
  assert.equal(choose('fast', false, true, true, false), 'server', 'no SR: pref cannot conjure it');
  // A dead browser engine beats any preference.
  assert.equal(choose('fast', true, true, true, true), 'server', 'fallback outranks the fast pref');
  assert.equal(choose(null, true, true, true, true), 'server');
  assert.equal(choose(null, true, true, false, true), 'browser', 'fallback needs the server engine too');
});

test('a stored preference is read at load and only valid values count', () => {
  // The module reads localStorage at evaluation time; a garbage value must not
  // become a pref (chooseEngine only understands fast/accurate/null).
  const t = load();
  assert.equal(t.window.LetsChatTranscribe.chooseEngine('garbage', false, true, true, false), 'server',
    'an unknown pref falls through to the automatic rules');
  assert.equal(t.window.LetsChatTranscribe.chooseEngine('garbage', true, true, true, false), 'browser');
});

test('nearBottom and why auto-follow cannot be derived from it post-insert (LC-847)', () => {
  const near = load().window.LetsChatTranscribe.nearBottom;
  // Pinned to the bottom: inside the 48px band.
  assert.equal(near({ scrollHeight: 1000, scrollTop: 700, clientHeight: 300 }), true);
  assert.equal(near({ scrollHeight: 1000, scrollTop: 660, clientHeight: 300 }), true, '40px off still counts as bottom');
  assert.equal(near({ scrollHeight: 1000, scrollTop: 640, clientHeight: 300 }), false, '60px off is scrolled up');
  // The LC-847 regression: the view sat AT the bottom, then one Accurate-mode
  // 5s clip appended a 120px batch. Measured after the insert (the only moment
  // the MutationObserver can run), the view is now 120px off the bottom, so a
  // nearBottom-gated auto-scroll refuses to follow - which is why onNewCaptions
  // scrolls on the followLive intent flag instead of re-deriving it here.
  assert.equal(near({ scrollHeight: 1120, scrollTop: 700, clientHeight: 300 }), false);
});

// LC-859: the server-side agent yields. A late joiner adopts an open session via
// GET .../transcript/active; when that reports agent_active, the client must NOT
// open its own per-client clip capture (the agent already transcribes every
// track). This drives the real adopt path and asserts on whether getUserMedia -
// the first thing the "server" engine does - is reached.
//
// A permissive Proxy stands in for every DOM node so the module's banner/toggle
// wiring runs without a real DOM; only the fetch routing, the mic spy, and the
// captured document listeners matter to the assertion.
function loadRich(opts) {
  const src = fs.readFileSync(path.join(__dirname, 'transcribe.js'), 'utf8');
  const stored = { 'lc-stt-engine': 'accurate' };
  const gum = { n: 0 };
  const docListeners = {};
  function stubEl() {
    return new Proxy(
      {},
      {
        get(_t, p) {
          if (p === 'classList') return { add() {}, remove() {}, toggle() {}, contains: () => false };
          if (p === 'style') return {};
          if (p === 'querySelector') return () => stubEl();
          if (p === 'querySelectorAll') return () => [];
          if (p === 'getAttribute') return () => null;
          if (p === 'hasAttribute') return () => false;
          if (p === 'closest') return () => null;
          if (['setAttribute', 'removeAttribute', 'appendChild', 'removeChild', 'remove',
            'replaceChildren', 'addEventListener', 'removeEventListener', 'focus', 'scrollIntoView', 'insertBefore'].includes(p)) {
            return () => {};
          }
          if (p === 'parentNode') return null;
          if (p === 'children') return [];
          return '';
        },
        set() { return true; },
      }
    );
  }
  const toasts = [];
  // LC-1129: diagnostics ride window.__lcWS (the shared chat socket); collect the
  // frames so a test can assert the voice_diag the admin voice log would render.
  const frames = [];
  // LC-1129: an optional Web Speech recognizer. When opts.withSR is set, SR is
  // present so chooseEngine picks the browser engine, and each instance is
  // captured so a test can fire its onresult / onerror the way the browser does.
  const recogInstances = [];
  function FakeSR() { recogInstances.push(this); }
  FakeSR.prototype.start = function () {};
  FakeSR.prototype.stop = function () {};
  // A MediaRecorder that records nothing but honours start/stop, so the success
  // path (getUserMedia resolves -> recordClip) runs without a real recorder.
  function FakeMR() {}
  FakeMR.prototype.start = function () {};
  FakeMR.prototype.stop = function () {};
  FakeMR.isTypeSupported = () => false;
  const window = {
    // No SpeechRecognition by default: with a server engine configured,
    // chooseEngine picks 'server' (the path that reaches getUserMedia), so the
    // suppression is what the assertion turns on rather than an engine-choice
    // accident. opts.withSR flips this to exercise the browser-engine paths.
    SpeechRecognition: opts.withSR ? FakeSR : undefined,
    MediaRecorder: FakeMR,
    LetsChatMedia: { audio: () => ({}) },
    __lcSessionRoom: 7,
    __lcWS: { send: (s) => frames.push(JSON.parse(s)) },
    __lcToast: (kind, msg) => { toasts.push([kind, msg]); },
    __lcNotify: (kind, msg) => { toasts.push([kind, msg]); },
    __lcS: (_k, fb) => fb,
  };
  // PMS-1632: track clears of the caption log so a test can assert a new call
  // starts fresh. querySelectorAll is here because the decoration setup reaches
  // for it (guarded by MutationObserver, absent in this sandbox).
  const captionLog = { clears: 0, replaceChildren() { this.clears += 1; }, querySelectorAll: () => [] };
  const document = {
    body: stubEl(),
    addEventListener(type, fn) { (docListeners[type] = docListeners[type] || []).push(fn); },
    querySelector: () => stubEl(),
    querySelectorAll: () => [],
    getElementById: (id) => (id === 'lc-caption-log' ? captionLog : stubEl()),
  };
  const fetch = (url) => {
    if (url.indexOf('/call/config') === 0) {
      return Promise.resolve({ ok: true, json: () => Promise.resolve({ sttServer: true }) });
    }
    if (url.indexOf('/transcript/active') !== -1) {
      return Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ transcript_id: 5, agent_active: !!opts.agentActive }),
      });
    }
    // Any other call (a clip POST would land here); harmless in this test.
    return Promise.resolve({ ok: true, text: () => Promise.resolve('') });
  };
  const navigator = {
    language: 'en-US',
    // getUserMedia behaviour is opts-driven. `gumFailures` unset (the default)
    // rejects every call - the adopt tests only care that it is REACHED, never
    // that it resolves. A number rejects that many leading calls, then resolves,
    // so LC-866's retry-after-Web-Speech-release can be exercised deterministically.
    mediaDevices: {
      getUserMedia: () => {
        gum.n += 1;
        if (opts.gumFailures == null || gum.n <= opts.gumFailures) {
          return Promise.reject({ name: 'NotReadableError', message: 'device busy' });
        }
        return Promise.resolve({ getTracks: () => [{ stop() {} }] });
      },
    },
  };
  // Controllable timers: transcribe.js schedules the LC-866 retry (250ms) and the
  // clip cadence (5s) via setTimeout. Collect them so a test fires the retry on
  // demand and no real timer outlives the test.
  const timers = [];
  const setTimeoutStub = (fn) => { timers.push(fn); return timers.length; };
  const runTimers = () => { timers.splice(0).forEach((fn) => fn()); };
  const sandbox = {
    window, document, navigator, fetch,
    localStorage: {
      getItem: (k) => (k in stored ? stored[k] : null),
      setItem: (k, v) => { stored[k] = String(v); },
    },
    console, setTimeout: setTimeoutStub, clearTimeout: () => {},
  };
  vm.runInNewContext(src, sandbox);
  const flush = async () => { for (let i = 0; i < 30; i += 1) await Promise.resolve(); };
  const fireDoc = (type, detail) => (docListeners[type] || []).forEach((fn) => fn({ type, detail }));
  return { fireDoc, flush, gum, toasts, runTimers, captionLog, frames, recogInstances };
}

test('LC-859: a late joiner adopting an agent-covered session does not open its own capture', async () => {
  const t = loadRich({ agentActive: true });
  await t.flush(); // let loadConfig() resolve sttServer=true
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush(); // adopt fetch -> agentActive -> startLocalCapture

  assert.equal(t.gum.n, 0, 'the server-clip capture is suppressed while the agent covers the room');
});

test('LC-859: without an agent, adopting an open session still opens per-client capture', async () => {
  const t = loadRich({ agentActive: false });
  await t.flush();
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush();

  assert.ok(t.gum.n >= 1, 'no agent -> the client captures its own clips as before');
});

test('PMS-1632: adopting a call session clears the stale caption log; re-adopting the same one does not', async () => {
  const t = loadRich({ agentActive: false });
  await t.flush();
  assert.equal(t.captionLog.clears, 0, 'nothing cleared before a session is active');

  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush();
  assert.equal(t.captionLog.clears, 1, 'adopting a session clears the previous call\'s lines so the new call starts fresh');

  // A second start event for the already-adopted session must NOT re-clear -
  // that is what keeps a reconnect to the same open call from wiping its own
  // captions.
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush();
  assert.equal(t.captionLog.clears, 1, 'the same active session does not re-clear');
});

test('LC-866: a mic acquire that fails right after the switch is retried, not swallowed', async () => {
  // gumFailures: 1 = the first getUserMedia rejects (Web Speech has not released
  // the device yet on a Fast -> Accurate switch), the retry succeeds.
  const t = loadRich({ agentActive: false, gumFailures: 1 });
  await t.flush();
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush();
  assert.equal(t.gum.n, 1, 'the first acquire failed');
  assert.deepEqual(t.toasts, [], 'no give-up toast while a retry is pending');

  t.runTimers(); // the 250ms retry fires
  await t.flush();
  assert.equal(t.gum.n, 2, 'the retry re-acquires the mic once Web Speech let go');
  assert.deepEqual(t.toasts, [], 'the retry succeeded, so nothing is surfaced');
});

test('LC-866: a mic that stays busy surfaces a toast instead of stopping silently', async () => {
  const t = loadRich({ agentActive: false, gumFailures: 99 });
  await t.flush();
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush();
  t.runTimers(); // the single retry fires and also fails
  await t.flush();

  assert.equal(t.gum.n, 2, 'it tried, retried once, then gave up (no infinite retry)');
  assert.equal(t.toasts.length, 1, 'the failure is surfaced, not swallowed');
  assert.equal(t.toasts[0][0], 'err');
  assert.match(t.toasts[0][1], /transcription/i);
});

// LC-1129: a browser-engine speech error (the recognizer's cloud is unreachable
// or the mic is blocked) is logged to the voice_events log as an error frame, in
// addition to the unchanged server-engine fallback.
test('LC-1129: a browser speech error sends an error diag', async () => {
  const t = loadRich({ agentActive: false, withSR: true });
  await t.flush(); // loadConfig -> sttServer=true
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush(); // adopt -> browser engine -> recog captured
  assert.ok(t.recogInstances.length >= 1, 'the browser recognizer started');

  t.recogInstances[0].onerror({ error: 'network' });

  const errs = t.frames.filter((f) => f.type === 'voice_diag' && f.detail === 'transcribe: speech error network');
  assert.equal(errs.length, 1, 'the speech error is logged once');
  assert.equal(errs[0].level, 'error', 'logged as an error, not a diag');
});

// LC-1129: the "talking but not picked up" watchdog. With the browser engine
// capturing and the mic live, a window with no caption logs exactly one error,
// and does not re-arm until the next caption arrives.
test('LC-1129: the silence watchdog fires once while the mic is live, then waits for a caption', async () => {
  const t = loadRich({ agentActive: false, withSR: true });
  await t.flush();
  t.fireDoc('lc:rtc-session-started', { room: 7 });
  await t.flush(); // browser capture + the silence watchdog armed

  t.runTimers(); // the no-caption window elapses
  await t.flush();
  const silence = t.frames.filter((f) => f.detail === 'transcribe: no captions in 60s (mic live)');
  assert.equal(silence.length, 1, 'one silence error');
  assert.equal(silence[0].level, 'error');

  // No caption yet, so it must NOT re-arm: another elapse logs nothing more.
  t.runTimers();
  await t.flush();
  assert.equal(t.frames.filter((f) => f.detail === 'transcribe: no captions in 60s (mic live)').length, 1,
    'a continuously-silent room logs at most one, not a stream');

  // A caption re-arms the watchdog; a later silent window trips it again.
  t.recogInstances[0].onresult({ resultIndex: 0, results: [] });
  await t.flush();
  t.runTimers();
  await t.flush();
  assert.equal(t.frames.filter((f) => f.detail === 'transcribe: no captions in 60s (mic live)').length, 2,
    'after a caption, the next silent window trips it again');
});

// LC-1131: the pure bleed-through gate. A browser Web Speech caption is dropped
// (not mis-attributed) when the local-mic VAD is active but our own echo-
// cancelled mic has not shown speech within the window - that caption came from
// another participant bleeding into the raw mic the Web Speech engine taps
// (e.g. a Firefox user whose own browser cannot transcribe). Fails open when the
// VAD is inactive so a surface with no working VAD never silences real speech.
test('LC-1131: bleedGateDrops matrix', () => {
  const drops = load().window.LetsChatTranscribe.bleedGateDrops;
  const now = 100000;
  // VAD inactive: never drop, whatever the timestamp (fail open).
  assert.equal(drops(false, 0, now, 3000), false, 'no VAD -> keep');
  assert.equal(drops(false, now - 999999, now, 3000), false);
  // VAD active + recent local speech: keep (it is plausibly ours).
  assert.equal(drops(true, now, now, 3000), false, 'spoke just now -> keep');
  assert.equal(drops(true, now - 2999, now, 3000), false, 'within window -> keep');
  assert.equal(drops(true, now - 3000, now, 3000), false, 'edge of window -> keep');
  // VAD active + stale (or never any) local speech: drop as bleed-through.
  assert.equal(drops(true, now - 3001, now, 3000), true, 'just past window -> drop');
  assert.equal(drops(true, 0, now, 3000), true, 'never spoke this session -> drop');
});

// LC-1133: the live transcript search helpers. capLineMatches decides which
// lines survive the filter; highlightHtml renders a line with the matched text
// wrapped in <mark>, escaping everything so a caption can never inject markup.
test('LC-1133: capLineMatches is a case-insensitive contains, empty = all', () => {
  const m = load().window.LetsChatTranscribe.capLineMatches;
  assert.equal(m('the Budget review', 'budget'), true, 'case-insensitive');
  assert.equal(m('the Budget review', 'BUDG'), true);
  assert.equal(m('hello world', 'xyz'), false);
  assert.equal(m('anything', ''), true, 'empty query matches every line');
});

test('LC-1133: highlightHtml wraps matches and escapes all text', () => {
  const h = load().window.LetsChatTranscribe.highlightHtml;
  // Empty query: escaped text, no marks.
  assert.equal(h('a < b & c', ''), 'a &lt; b &amp; c');
  // Match wrapped, surrounding text escaped, original case preserved.
  assert.equal(
    h('cut the Budget now', 'budget'),
    'cut the <mark class="lc-cap-hit">Budget</mark> now',
  );
  // Every occurrence; the matched span itself is escaped.
  assert.equal(
    h('<x> and <x>', '<x>'),
    '<mark class="lc-cap-hit">&lt;x&gt;</mark> and <mark class="lc-cap-hit">&lt;x&gt;</mark>',
  );
  // A caption that is pure markup cannot inject: no raw < reaches the output
  // outside an escaped entity.
  const out = h('<img src=x onerror=alert(1)>', 'zzz');
  assert.equal(out.indexOf('<img'), -1, 'no live tag');
  assert.ok(out.indexOf('&lt;img') === 0, 'escaped instead');
});
