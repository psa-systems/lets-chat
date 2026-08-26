import { expect, test } from 'bun:test';
import { Clipper } from '../src/clipper.js';

// 16 kHz mono, 1000 ms clips => 16000 samples per clip.
const mk = () => new Clipper(16000, 1, 1000);

test('emits a clip once CLIP_MS of audio has accumulated', () => {
  const c = mk();
  // 9 frames of 1600 samples = 14400 (< 16000): no clip yet.
  for (let i = 0; i < 9; i++) expect(c.push(new Int16Array(1600))).toBeNull();
  // 10th frame crosses the threshold.
  const clip = c.push(new Int16Array(1600));
  expect(clip).not.toBeNull();
  expect(clip!.samples.length).toBe(16000);
  expect(Math.round(clip!.durationMs)).toBe(1000);
});

test('duration reflects the real sample count, and buffers reset between clips', () => {
  const c = mk();
  // One oversized frame (2s) yields a clip of exactly that length.
  const clip = c.push(new Int16Array(32000));
  expect(clip).not.toBeNull();
  expect(clip!.samples.length).toBe(32000);
  expect(Math.round(clip!.durationMs)).toBe(2000);
  // After draining, a fresh sub-threshold push does not emit.
  expect(c.push(new Int16Array(100))).toBeNull();
});

test('flush emits the tail, then nothing', () => {
  const c = mk();
  c.push(new Int16Array(500));
  const tail = c.flush();
  expect(tail!.samples.length).toBe(500);
  expect(c.flush()).toBeNull();
});

test('a muted participant (no frames) yields zero clips (LC-626)', () => {
  const c = mk();
  // No push() calls at all: nothing accumulates, flush is empty.
  expect(c.flush()).toBeNull();
});

test('an empty frame is ignored', () => {
  const c = mk();
  expect(c.push(new Int16Array(0))).toBeNull();
  expect(c.flush()).toBeNull();
});
