// LC-632: run with `just test-js` (node --test). devices.js is a browser IIFE
// that touches window/document at load, so it cannot be required in Node; this
// asserts on its source instead - a regression guard that the call-devices
// picker keeps the dark-themed, icon-labelled treatment and never falls back to
// the inverted light default that this ticket fixed.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const src = fs.readFileSync(path.join(__dirname, 'devices.js'), 'utf8');

test('call-devices picker uses dark theme tokens, not the inverted light defaults', () => {
  assert.ok(!/bg-white/.test(src), 'picker must not hardcode bg-white');
  assert.ok(!/(border|text|bg)-slate-\d/.test(src), 'picker must not use slate palette classes');
  assert.ok(/bg-surface-elevated/.test(src), 'dialog uses the elevated surface token');
  assert.ok(/bg-surface-sunken/.test(src), 'the selects use the sunken surface token');
  assert.ok(/text-content\b/.test(src), 'text uses the content token');
});

test('device types render as icons with a chevron affordance', () => {
  for (const kind of ['audioinput', 'videoinput', 'audiooutput']) {
    assert.match(src, new RegExp(kind + ":\\s*'<svg"), kind + ' has an icon glyph');
  }
  assert.ok(/var CHEVRON =/.test(src), 'a chevron affordance is defined');
  assert.ok(/appearance-none/.test(src), 'the select drops its native arrow for the chevron');
});
