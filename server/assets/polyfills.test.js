// LC-1162: run with `just test-js` (node --test). polyfills.js augments
// HTMLFormElement.prototype at load, so (like sw.test.js) evaluate the source in
// a VM sandbox over a tiny fake form/input and assert: it defines requestSubmit
// when missing, fires a real submit event (the path htmx relies on), clicks an
// explicit submitter, rejects a foreign submitter, and never overrides a native
// method.
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// Build a sandbox with a minimal HTMLFormElement + document, load polyfills.js
// into it, and hand back the pieces a test drives. `native: true` pre-defines a
// requestSubmit so the no-override branch can be exercised.
function loadPolyfill(opts = {}) {
  const counters = { nativeCalled: 0 };

  class HTMLFormElement {
    constructor() {
      this.children = [];
      this.submitListeners = [];
    }
    appendChild(node) {
      node.form = this;
      this.children.push(node);
      return node;
    }
    removeChild(node) {
      this.children = this.children.filter((n) => n !== node);
      return node;
    }
    addEventListener(type, fn) {
      if (type === 'submit') this.submitListeners.push(fn);
    }
    dispatchSubmit() {
      this.submitListeners.forEach((fn) => fn({ type: 'submit' }));
    }
  }
  if (opts.native) {
    HTMLFormElement.prototype.requestSubmit = function () {
      counters.nativeCalled += 1;
    };
  }

  const makeInput = () => ({
    type: '',
    hidden: false,
    form: null,
    click() {
      if (this.type === 'submit' && this.form) this.form.dispatchSubmit();
    },
  });

  const sandbox = {
    HTMLFormElement,
    document: { createElement: () => makeInput() },
    window: { DOMException: function (msg, name) { this.message = msg; this.name = name; } },
  };
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(path.join(__dirname, 'polyfills.js'), 'utf8'), sandbox);
  return { HTMLFormElement, makeInput, counters };
}

test('LC-1162: defines requestSubmit when missing and fires a submit event', () => {
  const { HTMLFormElement } = loadPolyfill();
  const form = new HTMLFormElement();
  let submits = 0;
  form.addEventListener('submit', () => { submits += 1; });

  assert.equal(typeof form.requestSubmit, 'function', 'requestSubmit was polyfilled');
  form.requestSubmit();
  assert.equal(submits, 1, 'one submit event fired');
  assert.equal(form.children.length, 0, 'the temporary submit control was cleaned up');
});

test('LC-1162: requestSubmit(submitter) clicks the given submitter', () => {
  const { HTMLFormElement, makeInput } = loadPolyfill();
  const form = new HTMLFormElement();
  let submits = 0;
  form.addEventListener('submit', () => { submits += 1; });
  const btn = makeInput();
  btn.type = 'submit';
  form.appendChild(btn);

  form.requestSubmit(btn);
  assert.equal(submits, 1, 'clicking the submitter dispatched submit');
});

test('LC-1162: a submitter owned by another form throws', () => {
  const { HTMLFormElement, makeInput } = loadPolyfill();
  const form = new HTMLFormElement();
  const foreign = makeInput();
  foreign.type = 'submit';
  foreign.form = new HTMLFormElement();
  assert.throws(() => form.requestSubmit(foreign));
});

test('LC-1162: a native requestSubmit is left untouched', () => {
  const { HTMLFormElement, counters } = loadPolyfill({ native: true });
  const form = new HTMLFormElement();
  let submits = 0;
  form.addEventListener('submit', () => { submits += 1; });

  form.requestSubmit();
  assert.equal(counters.nativeCalled, 1, 'the native method was used');
  assert.equal(submits, 0, 'the polyfill fallback path was not taken');
});
