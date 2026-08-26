import { expect, test } from 'bun:test';
import { encodeWav } from '../src/wav.js';

const ascii = (b: Uint8Array, off: number, len: number) =>
  String.fromCharCode(...b.subarray(off, off + len));

test('encodeWav writes a 44-byte header then the samples', () => {
  const samples = Int16Array.from([0, 1, -1, 32767, -32768]);
  const out = encodeWav(samples, 16000, 1);
  expect(out.length).toBe(44 + samples.length * 2);
  expect(ascii(out, 0, 4)).toBe('RIFF');
  expect(ascii(out, 8, 4)).toBe('WAVE');
  expect(ascii(out, 12, 4)).toBe('fmt ');
  expect(ascii(out, 36, 4)).toBe('data');
});

test('encodeWav header fields are correct little-endian', () => {
  const samples = Int16Array.from([7, -7]);
  const out = encodeWav(samples, 48000, 2);
  const dv = new DataView(out.buffer);
  expect(dv.getUint32(4, true)).toBe(36 + samples.length * 2); // file size - 8
  expect(dv.getUint16(20, true)).toBe(1); // PCM
  expect(dv.getUint16(22, true)).toBe(2); // channels
  expect(dv.getUint32(24, true)).toBe(48000); // sample rate
  expect(dv.getUint32(28, true)).toBe(48000 * 2 * 2); // byte rate
  expect(dv.getUint16(32, true)).toBe(4); // block align
  expect(dv.getUint16(34, true)).toBe(16); // bits per sample
  expect(dv.getUint32(40, true)).toBe(samples.length * 2); // data length
  // Samples round-trip.
  expect(dv.getInt16(44, true)).toBe(7);
  expect(dv.getInt16(46, true)).toBe(-7);
});
