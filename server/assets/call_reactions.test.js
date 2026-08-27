// LC-825: run with `node --test server/assets/call_reactions.test.js`
// (or `just test-js`). Covers the pure helpers behind the floating call
// reactions: quick-row ordering, the client-side emoji gate, lane spread, and
// the coalesced screen-reader announcement.
const test = require('node:test');
const assert = require('node:assert/strict');
const rx = require('./call_reactions.js');

test('trayOrder puts frequent emoji first, de-duplicates, and caps at 8', () => {
  const out = rx.trayOrder(['🔥', '👍', '🔥'], rx.DEFAULTS, 8);
  assert.equal(out.length, 8);
  assert.deepEqual(out.slice(0, 2), ['🔥', '👍']);
  assert.equal(new Set(out).size, out.length, 'no duplicates');
  // No frequent list at all: the defaults, in order.
  assert.deepEqual(rx.trayOrder(null, rx.DEFAULTS, 8), rx.DEFAULTS);
  // Non-strings and blanks are ignored rather than rendered as cells.
  assert.deepEqual(rx.trayOrder([null, '', 42, '🎉'], ['👍'], 8), ['🎉', '👍']);
});

test('isLikelyEmoji accepts emoji sequences and rejects text, markup, shortcodes', () => {
  for (const ok of ['👍', '❤️', '👍🏽', '🇺🇸', '👨‍👩‍👧‍👦', ' 🎉 ']) {
    assert.equal(rx.isLikelyEmoji(ok), true, ok);
  }
  for (const bad of ['', '   ', 'a', '👍x', ':smile:', '<b>', '👍 👍', null, 7, '😀'.repeat(20)]) {
    assert.equal(rx.isLikelyEmoji(bad), false, String(bad));
  }
});

test('laneX spreads consecutive floats across lanes and stays inside the band', () => {
  const xs = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => rx.laneX(i, 8, 0));
  for (let i = 1; i < xs.length; i++) assert.ok(xs[i] > xs[i - 1], 'lanes ascend');
  assert.equal(rx.laneX(8, 8, 0), xs[0], 'wraps round-robin');
  // Extreme jitter never leaves the visible band.
  assert.ok(rx.laneX(0, 8, -1) >= 6);
  assert.ok(rx.laneX(7, 8, 1) <= 94);
});

test('coalesce announces one reaction, or a burst as "and N others"', () => {
  const labels = { one: '%name% reacted %emoji%', many: '%name% and %n% others reacted %emoji%' };
  assert.equal(rx.coalesce([{ name: 'Alice', emoji: '👍' }], labels), 'Alice reacted 👍');
  const burst = [
    { name: 'Alice', emoji: '👍' },
    { name: 'Bob', emoji: '🎉' },
    { name: 'Cara', emoji: '👍' },
  ];
  assert.equal(rx.coalesce(burst, labels), 'Alice and 2 others reacted 👍');
  assert.equal(rx.coalesce([], labels), '');
});
