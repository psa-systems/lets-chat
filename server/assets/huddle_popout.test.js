// LC-832: run with `just test-js` (node --test). huddle_popout.js is a browser
// IIFE, not a module: load the source and evaluate it in a VM sandbox over the
// shared DOM stub (test_dom.js), the same shape sw.test.js uses. Covers the
// floatOut() entry point (the automatic path voice.js takes on a swap), the
// setup it shares with popOut(), that popOut() keeps its Picture-in-Picture
// behaviour, and that release() leaves no floating panel behind.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

// A dock as the server renders it: the root, its bar, the Pop out button, and
// one <video> tile whose playback the float must not silence.
function makeDock(roomId) {
  const root = new El('div');
  root.setAttribute('data-lc-voice-root', '');
  root.setAttribute('data-lc-huddle', '');
  root.setAttribute('data-room-id', String(roomId));
  root.className = 'lc-huddle';
  const bar = new El('div');
  bar.className = 'lc-huddle-bar';
  const btn = new El('button');
  btn.setAttribute('data-lc-huddle-popout', '');
  const join = new El('button');
  join.setAttribute('data-lc-voice-join', '');
  const video = new El('video');
  bar.appendChild(btn);
  bar.appendChild(join);
  root.appendChild(bar);
  root.appendChild(video);
  return root;
}

function load(opts) {
  const src = fs.readFileSync(path.join(__dirname, 'huddle_popout.js'), 'utf8');
  const body = new El('body');
  const store = new Map();
  const pipCalls = [];
  const document = {
    body,
    documentElement: { attributes: [] },
    styleSheets: [],
    createElement: (t) => new El(t),
    addEventListener: () => {},
    querySelector: (sel) => body.querySelector(sel),
    querySelectorAll: (sel) => body.querySelectorAll(sel),
  };
  const window = {
    innerWidth: 1280,
    innerHeight: 800,
    __lcS: (k, fb) => fb,
  };
  if (opts && opts.pip) {
    window.documentPictureInPicture = {
      requestWindow: (o) => { pipCalls.push(o); return opts.pip(); },
    };
  }
  const sandbox = {
    window,
    document,
    localStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
    },
    console: { warn: () => {}, log: () => {}, error: () => {} },
    setTimeout,
    clearTimeout,
    Promise,
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(src, sandbox);
  return { api: window.LetsChatHuddlePopout, body, document, window, pipCalls, store };
}

// A PiP window that never resolves: proof the branch was not entered, since a
// floatOut() that reached it would hang instead of floating.
const pendingPip = () => new Promise(() => {});

test('LC-832: floatOut floats the live dock and never touches Picture-in-Picture', () => {
  const { api, body, pipCalls } = load({ pip: pendingPip });
  const root = makeDock(7);
  body.appendChild(root);

  api.floatOut(root);

  assert.equal(pipCalls.length, 0, 'requestWindow must not run outside a click activation');
  assert.equal(api.isPopped(), true, 'the live binding is preserved for voice.js scan()');
  assert.equal(api.roomId(), '7');
  assert.equal(root.parentNode, body, 'the dock is re-parented outside #main');
  assert.equal(root.classList.contains('lc-huddle--float'), true);
});

test('LC-832: floatOut leaves the bring-back placeholder popOut leaves', () => {
  const { api, body } = load({ pip: pendingPip });
  const host = new El('div');           // stands in for the room page foot
  body.appendChild(host);
  const root = makeDock(7);
  host.appendChild(root);

  api.floatOut(root);

  const ph = body.querySelector('[data-lc-huddle-placeholder][data-room-id="7"]');
  assert.ok(ph, 'a placeholder marks where the dock was');
  assert.equal(ph.parentNode, host);
  assert.equal(root.parentNode, body);
});

test('LC-832: floatOut on a dock the swap already detached still floats, and the room re-render becomes the placeholder', () => {
  const { api, body } = load({ pip: pendingPip });
  const root = makeDock(7);             // never attached: htmx removed it
  api.floatOut(root);

  assert.equal(api.isPopped(), true);
  assert.equal(body.querySelectorAll('[data-lc-huddle-placeholder]').length, 0);

  // Navigating back to room 7 renders its own dock; it becomes the placeholder.
  const host = new El('div');
  body.appendChild(host);
  const fresh = makeDock(7);
  host.appendChild(fresh);
  api.adoptPageDock(fresh);
  assert.equal(fresh.parentNode, null, 'no second dock for the same room');
  assert.ok(body.querySelector('[data-lc-huddle-placeholder][data-room-id="7"]'));

  // A different room's dock is marked busy instead (one session per tab).
  const other = makeDock(9);
  body.appendChild(other);
  api.adoptPageDock(other);
  assert.equal(other.hasAttribute('data-lc-huddle-busy'), true);
  assert.equal(other.querySelector('[data-lc-voice-join]').hasAttribute('disabled'), true);
});

test('LC-832: floatOut re-plays media the detach paused', () => {
  const { api, body } = load({ pip: pendingPip });
  const root = makeDock(7);
  const video = root.querySelector('video');
  video.paused = true;                  // as the removal from the document left it
  api.floatOut(root);
  assert.equal(video.playCalls, 1, 'a floated call must not go silent');
  assert.equal(body.contains(root), true);

  // Already playing: nothing to resume, no redundant play().
  const second = load({ pip: pendingPip });
  const root2 = makeDock(8);
  const video2 = root2.querySelector('video');
  video2.paused = false;
  second.api.floatOut(root2);
  assert.equal(video2.playCalls, 0);
});

test('LC-832: floatOut is a no-op once the dock is already lifted out', () => {
  const { api } = load({ pip: pendingPip });
  const root = makeDock(7);
  api.floatOut(root);
  const other = makeDock(9);
  api.floatOut(other);
  assert.equal(api.roomId(), '7', 'the live session keeps the panel');
  assert.equal(other.classList.contains('lc-huddle--float'), false);
});

test('LC-832: release() takes the floating panel down - no orphan survives the call', () => {
  const { api, body } = load({ pip: pendingPip });
  const root = makeDock(7);
  api.floatOut(root);
  assert.equal(body.contains(root), true);

  api.release();                        // voice.js leave() -> releasePopout()

  assert.equal(api.isPopped(), false);
  assert.equal(api.roomId(), null);
  assert.equal(body.contains(root), false, 'no panel is left over a call that ended');
  assert.equal(root.classList.contains('lc-huddle--float'), false);
});

test('LC-832: release() re-docks into the placeholder when the call ends on its own page', () => {
  const { api, body } = load({ pip: pendingPip });
  const host = new El('div');
  body.appendChild(host);
  const root = makeDock(7);
  host.appendChild(root);
  api.floatOut(root);

  api.release();

  assert.equal(root.parentNode, host, 'the dock goes back where it came from');
  assert.equal(body.querySelectorAll('[data-lc-huddle-placeholder]').length, 0);
  assert.equal(api.isPopped(), false);
});

test('LC-822 regression: the Pop out click still prefers a Picture-in-Picture window', async () => {
  const pip = {
    document: {
      head: new El('head'),
      body: new El('body'),
      documentElement: new El('html'),
      createElement: (t) => new El(t),
    },
    addEventListener: () => {},
    close: () => {},
  };
  const { api, body, pipCalls } = load({ pip: () => Promise.resolve(pip) });
  const root = makeDock(7);
  body.appendChild(root);

  api.popOut(root);
  await Promise.resolve();
  await Promise.resolve();

  assert.equal(pipCalls.length, 1, 'the manual path keeps its PiP behaviour');
  assert.equal(root.parentNode, pip.document.body);
  assert.equal(root.classList.contains('lc-huddle--popout'), true);
  assert.equal(root.classList.contains('lc-huddle--float'), false);
  assert.equal(api.isPopped(), true);
});

test('LC-822 regression: Pop out falls back to the floating panel without PiP support', () => {
  const { api, body } = load({});       // no window.documentPictureInPicture
  const root = makeDock(7);
  body.appendChild(root);
  api.popOut(root);
  assert.equal(root.classList.contains('lc-huddle--float'), true);
  assert.equal(api.isPopped(), true);
});
