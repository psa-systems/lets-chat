// LC-815: encode raw PCM into a WAV container.
//
// The agent captures decoded 16-bit PCM audio frames from LiveKit and has to
// hand lets-chat's STT a self-describing clip. WAV is the least-effort container
// every STT provider (OpenAI Whisper, Deepgram) accepts, and it needs no codec
// dependency - just a 44-byte header in front of the little-endian samples. This
// keeps the agent a dumb media bridge: no encoding libraries, no transcoding.

/** Bytes of a canonical 44-byte PCM WAV header + the samples, little-endian. */
export function encodeWav(
  samples: Int16Array,
  sampleRate: number,
  channels: number,
): Uint8Array {
  const dataLen = samples.length * 2; // 16-bit
  const buf = new ArrayBuffer(44 + dataLen);
  const view = new DataView(buf);

  const writeAscii = (offset: number, s: string) => {
    for (let i = 0; i < s.length; i++) view.setUint8(offset + i, s.charCodeAt(i));
  };

  writeAscii(0, 'RIFF');
  view.setUint32(4, 36 + dataLen, true); // file size - 8
  writeAscii(8, 'WAVE');
  writeAscii(12, 'fmt ');
  view.setUint32(16, 16, true); // PCM fmt chunk size
  view.setUint16(20, 1, true); // audio format = PCM
  view.setUint16(22, channels, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * channels * 2, true); // byte rate
  view.setUint16(32, channels * 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  writeAscii(36, 'data');
  view.setUint32(40, dataLen, true);

  for (let i = 0; i < samples.length; i++) {
    view.setInt16(44 + i * 2, samples[i], true);
  }
  return new Uint8Array(buf);
}
