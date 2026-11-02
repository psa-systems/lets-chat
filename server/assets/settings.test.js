// LC-1106: the settings deep-links in the persistent shell are boosted now, so
// following one mid-call keeps the live call and socket instead of reloading
// the page. A boosted swap pushes the new URL only after this page's scripts
// run, so location.hash is not reliably set when tabs.js reads it; settings.js
// records the target tab in sessionStorage on click, and tabs.js falls back to
// it, so the right tab opens regardless of htmx's hash timing. These cover both
// halves of that chain.
//
// Run with `just test-js` (node --test). settings.js and tabs.js are browser
// IIFEs that reach for `document` at load, so evaluate the source in a VM
// sandbox over the shared DOM stub (test_dom.js), the idiom the other suites use.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

function load() {
  const body = new El('body');
  const store = new Map();
  const sessionStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
  };
  const location = { hash: '' };
  const history = { replaceState: () => {} };
  const document = {
    body,
    readyState: 'complete',
    addEventListener: () => {},
    removeEventListener: () => {},
    querySelector: (sel) => body.querySelector(sel),
    querySelectorAll: (sel) => body.querySelectorAll(sel),
    createElement: (t) => new El(t),
  };
  const window = {
    location,
    history,
    addEventListener: () => {},
    __lcNotify: () => {},
  };
  const sandbox = {
    window,
    document,
    location,
    history,
    sessionStorage,
    console: { warn() {}, log() {}, error() {} },
    setTimeout,
    clearTimeout,
    Promise,
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  // tabs.js first: settings.js's init() calls window.lcInitTabs.
  vm.runInContext(fs.readFileSync(path.join(__dirname, 'tabs.js'), 'utf8'), sandbox);
  vm.runInContext(fs.readFileSync(path.join(__dirname, 'settings.js'), 'utf8'), sandbox);
  return { window, document, body, store, location, sessionStorage };
}

// A minimal settings tablist as the server renders it: two tabs, two panels.
function makeTablist(document) {
  const root = new El('div');
  root.setAttribute('data-lc-settings', '');
  ['profile', 'appearance', 'privacy'].forEach((key) => {
    const tab = new El('button');
    tab.setAttribute('data-lc-tab', key);
    tab.setAttribute('aria-selected', key === 'profile' ? 'true' : 'false');
    root.appendChild(tab);
    const panel = new El('section');
    panel.setAttribute('data-lc-tabpanel', key);
    panel.hidden = key !== 'profile';
    root.appendChild(panel);
  });
  return root;
}

function selected(root) {
  const on = root
    .querySelectorAll('[data-lc-tab]')
    .filter((t) => t.getAttribute('aria-selected') === 'true');
  return on.length === 1 ? on[0].getAttribute('data-lc-tab') : null;
}

test('LC-1106: a /settings# deep-link click remembers its tab in sessionStorage', () => {
  const { window, store } = load();
  window.__lcSettingsRememberTab('/settings#appearance');
  assert.equal(store.get('lc-settings-tab'), 'appearance');
});

test('LC-1106: remembering ignores a non-settings hash and a hash-less href', () => {
  const { window, store } = load();
  window.__lcSettingsRememberTab('/room/5#jump');
  window.__lcSettingsRememberTab('/settings');
  window.__lcSettingsRememberTab('');
  assert.equal(store.has('lc-settings-tab'), false, 'only /settings# links are remembered');
});

test('LC-1106: tabs.js opens the remembered tab when the hash is empty after a boosted swap', () => {
  const { window, document, body, sessionStorage, location } = load();
  // What the deep-link click wrote before the boosted navigation; htmx has not
  // (yet) set the hash when the swapped-in settings page initialises its tabs.
  sessionStorage.setItem('lc-settings-tab', 'appearance');
  location.hash = '';
  const root = makeTablist(document);
  body.appendChild(root);

  window.lcInitTabs(root, 'lc-settings-tab');

  assert.equal(selected(root), 'appearance', 'the remembered tab is selected, not the first');
});

test('LC-1106: a real location.hash still wins over the remembered tab', () => {
  const { window, document, body, sessionStorage, location } = load();
  sessionStorage.setItem('lc-settings-tab', 'appearance');
  location.hash = '#privacy';
  const root = makeTablist(document);
  body.appendChild(root);

  window.lcInitTabs(root, 'lc-settings-tab');

  assert.equal(selected(root), 'privacy', 'the URL hash takes precedence when it names a tab');
});
