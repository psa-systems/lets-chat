// LC-815: slice a participant's continuous PCM stream into ~CLIP_MS clips.
//
// LiveKit delivers audio as a stream of small frames (typically 10ms). lets-chat
// transcribes short clips (the browser path uses a ~5s MediaRecorder cadence,
// CLIP_MS), so the agent buffers frames per participant and emits one clip each
// time it has accumulated CLIP_MS of audio. One Clipper per participant identity;
// attribution is the caller's job (it keys Clippers by identity).

export interface Clip {
  /** Interleaved 16-bit PCM for this clip. */
  samples: Int16Array;
  /** Real spoken length of the clip, from the sample count. */
  durationMs: number;
}

export class Clipper {
  private readonly threshold: number; // samples (all channels) per clip
  private buffered: Int16Array[] = [];
  private count = 0;

  constructor(
    private readonly sampleRate: number,
    private readonly channels: number,
    clipMs: number,
  ) {
    // Interleaved sample count for clipMs of audio across all channels.
    this.threshold = Math.max(1, Math.round((sampleRate * channels * clipMs) / 1000));
  }

  /** Feed one decoded frame. Returns a clip once CLIP_MS has accumulated. */
  push(frame: Int16Array): Clip | null {
    if (frame.length === 0) return null;
    this.buffered.push(frame);
    this.count += frame.length;
    if (this.count >= this.threshold) return this.drain();
    return null;
  }

  /** Emit whatever remains (e.g. when the participant leaves), or null if empty. */
  flush(): Clip | null {
    if (this.count === 0) return null;
    return this.drain();
  }

  private drain(): Clip {
    const samples = new Int16Array(this.count);
    let offset = 0;
    for (const f of this.buffered) {
      samples.set(f, offset);
      offset += f.length;
    }
    const durationMs = (samples.length / this.channels / this.sampleRate) * 1000;
    this.buffered = [];
    this.count = 0;
    return { samples, durationMs };
  }
}
