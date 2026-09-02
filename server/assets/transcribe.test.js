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
            'addEventListener', 'removeEventListener', 'focus', 'scrollIntoView', 'insertBefore'].includes(p)) {
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
  // A MediaRecorder that records nothing but honours start/stop, so the success
  // path (getUserMedia resolves -> recordClip) runs without a real recorder.
  function FakeMR() {}
  FakeMR.prototype.start = function () {};
  FakeMR.prototype.stop = function () {};
  FakeMR.isTypeSupported = () => false;
  const window = {
    // No SpeechRecognition: with a server engine configured, chooseEngine picks
    // 'server' (the path that reaches getUserMedia), so the suppression is what
    // the assertion turns on rather than an engine-choice accident.
    MediaRecorder: FakeMR,
    LetsChatMedia: { audio: () => ({}) },
    __lcSessionRoom: 7,
    __lcToast: (kind, msg) => { toasts.push([kind, msg]); },
    __lcS: (_k, fb) => fb,
  };
  const document = {
    body: stubEl(),
    addEventListener(type, fn) { (docListeners[type] = docListeners[type] || []).push(fn); },
    querySelector: () => stubEl(),
    querySelectorAll: () => [],
    getElementById: () => stubEl(),
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
  return { fireDoc, flush, gum, toasts, runTimers };
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
