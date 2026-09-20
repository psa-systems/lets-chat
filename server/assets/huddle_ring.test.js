// LC-945 regression: run with `just test-js` (node --test). huddle_ring.js is
// a browser IIFE with no exported API, so the test evaluates the source in a
// VM sandbox over the shared DOM stub (test_dom.js), same idiom as
// huddle_popout.test.js and call.test.js. Drives the module the way the page
// does: an OOB-appended bus node followed by the htmx:wsAfterMessage event
// (the MutationObserver path is stubbed inert, same as call.test.js, since
// the stub has no real mutation delivery). Pins the LC-945 fix: the ring's
// three strings must come from window.__lcI18n via window.__lcS(key,
// fallback), not the hardcoded fallback, under a non-English locale.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const { El } = require('./test_dom.js');

function load(lcS) {
  const src = fs.readFileSync(path.join(__dirname, 'huddle_ring.js'), 'utf8');

  const body = new El('body');
  const bodyListeners = {};
  body.addEventListener = (type, fn) => {
    (bodyListeners[type] = bodyListeners[type] || []).push(fn);
  };
  body.dispatchEvent = (evt) => {
    (bodyListeners[evt.type] || []).forEach((fn) => fn(evt));
  };

  const bus = new El('div');
  bus.setAttribute('id', 'lc-huddle-ring-bus');
  body.appendChild(bus);

  const root = new El('div');
  root.setAttribute('data-lc-huddle-ring-root', '');
  body.appendChild(root);

  const document = {
    body,
    getElementById: (id) => (id === 'lc-huddle-ring-bus' ? bus : null),
    querySelector: (sel) => body.querySelector(sel),
    querySelectorAll: (sel) => body.querySelectorAll(sel),
    createElement: (t) => new El(t),
    addEventListener: () => {},
  };

  const window = { __lcS: lcS, __lcSessionRoom: null };

  const sandbox = {
    window,
    document,
    MutationObserver: class { observe() {} disconnect() {} },
    console: { warn: () => {}, log: () => {}, error: () => {} },
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(src, sandbox);

  return { body, bus, root };
}

function ring(bus, attrs) {
  const node = new El('div');
  node.setAttribute('data-lc-huddle-ring', '');
  Object.keys(attrs).forEach((k) => node.setAttribute(k, attrs[k]));
  bus.appendChild(node);
}

test('LC-945 regression: the ring notification renders catalog strings under a non-English locale', () => {
  const catalog = {
    huddleRingStarted: 'inicio un huddle en',
    huddleRingJoin: 'Unirse',
    huddleRingIgnore: 'Ignorar',
  };
  const { body, bus, root } = load((k, fb) => (k in catalog ? catalog[k] : fb));

  ring(bus, {
    'data-room-id': '7',
    'data-room-name': 'General',
    'data-starter-id': '3',
    'data-starter-name': 'Ana',
  });
  // Same event the page's htmx OOB swap fires, which the module drains on.
  body.dispatchEvent({ type: 'htmx:wsAfterMessage' });

  const box = root.querySelector('.lc-huddle-ring');
  assert.ok(box, 'the ring banner rendered');
  const text = box.querySelector('.lc-huddle-ring-text').textContent;
  assert.ok(text.includes(catalog.huddleRingStarted), 'the started text must come from the catalog');
  assert.ok(!text.includes('started a huddle in'), 'must not fall back to the hardcoded English text');

  const join = box.querySelector('[data-lc-huddle-ring-join]');
  assert.equal(join.textContent, catalog.huddleRingJoin);
  const ignore = box.querySelector('[data-lc-huddle-ring-ignore]');
  assert.equal(ignore.textContent, catalog.huddleRingIgnore);
});

test('LC-945 regression: falls back to the English text only when the catalog has no entry', () => {
  const { body, bus, root } = load((_k, fb) => fb);

  ring(bus, {
    'data-room-id': '9',
    'data-room-name': 'Support',
    'data-starter-id': '2',
    'data-starter-name': 'Sam',
  });
  body.dispatchEvent({ type: 'htmx:wsAfterMessage' });

  const box = root.querySelector('.lc-huddle-ring');
  const text = box.querySelector('.lc-huddle-ring-text').textContent;
  assert.ok(text.includes('started a huddle in'));
  assert.equal(box.querySelector('[data-lc-huddle-ring-join]').textContent, 'Join');
  assert.equal(box.querySelector('[data-lc-huddle-ring-ignore]').textContent, 'Ignore');
});
