// LC-815: the sidecar transcription agent (LC-810 stage 3).
//
// A LiveKit Agents worker. lets-chat dispatches it into an SFU huddle (LC-814)
// with job metadata {transcript_id, base_url}. The agent joins under a reserved
// `agent-` identity (hidden from the roster), subscribes to every participant's
// audio track, slices each stream into ~CLIP_MS clips (Clipper), encodes each as
// WAV (encodeWav), and POSTs it to lets-chat's trusted agent-clip route (LC-813)
// attributed to the track's participant. It runs NO STT itself.
//
// The pure pieces (Clipper, encodeWav, callback URL/headers, config parsing) are
// unit-tested (bun test). This file is the LiveKit-SDK wiring, verified at
// staging (stage 5) against a live LiveKit + a real dispatch.

import {
  type JobContext,
  WorkerOptions,
  cli,
  defineAgent,
} from '@livekit/agents';
import {
  AudioStream,
  RemoteParticipant,
  RemoteTrack,
  RemoteTrackPublication,
  RoomEvent,
  TrackKind,
} from '@livekit/rtc-node';

import { Clipper } from './clipper.js';
import { encodeWav } from './wav.js';
import { postClip } from './callback.js';
import { loadConfig, parseJobMeta, type AgentConfig, type JobMeta } from './config.js';
import { agentIdentity } from './identity.js';

const config: AgentConfig = loadConfig();

function log(...args: unknown[]) {
  // eslint-disable-next-line no-console
  console.log('[transcription-agent]', ...args);
}

/**
 * Pump one participant's audio track: decode frames, clip at CLIP_MS, and POST
 * each clip attributed to the participant. Runs until the track ends (unsubscribe
 * / leave), then flushes the tail so the last partial clip is not lost.
 */
async function captureTrack(
  meta: JobMeta,
  participant: RemoteParticipant,
  track: RemoteTrack,
) {
  const speakerId = participant.identity;
  const stream = new AudioStream(track);
  let clipper: Clipper | null = null;
  let sampleRate = 0;
  let channels = 0;

  const send = async (samples: Int16Array, durationMs: number) => {
    const body = encodeWav(samples, sampleRate, channels);
    try {
      const status = await postClip({
        baseUrl: meta.baseUrl,
        transcriptId: meta.transcriptId,
        speakerId,
        token: config.callbackToken,
        body,
        durationSecs: durationMs / 1000,
      });
      if (status >= 400) log('clip rejected', { speakerId, status });
    } catch (e) {
      log('clip post failed', { speakerId, error: (e as Error).message });
    }
  };

  for await (const frame of stream) {
    // A muted participant publishes no audio track, so this loop never runs for
    // them (LC-626): no track -> no frames -> no clip. Nothing to special-case.
    if (!clipper) {
      sampleRate = frame.sampleRate;
      channels = frame.channels;
      clipper = new Clipper(sampleRate, channels, config.clipMs);
    }
    const samples = new Int16Array(
      frame.data.buffer,
      frame.data.byteOffset,
      frame.samplesPerChannel * frame.channels,
    );
    const clip = clipper.push(samples);
    if (clip) await send(clip.samples, clip.durationMs);
  }

  // Track ended (unsubscribe / leave): flush the last partial clip so it is not
  // lost. Only possible once we have seen a frame (hence a known format).
  const tail = clipper?.flush();
  if (tail) {
    log('track ended, flushing tail', { speakerId, tailSamples: tail.samples.length });
    await send(tail.samples, tail.durationMs);
  }
}

export default defineAgent({
  entry: async (ctx: JobContext) => {
    const meta = parseJobMeta(ctx.job.metadata);
    log('dispatched', { room: ctx.room?.name, transcriptId: meta.transcriptId });

    // Subscribe to remote audio tracks as they appear (covers mid-call joiners).
    ctx.room.on(
      RoomEvent.TrackSubscribed,
      (track: RemoteTrack, _pub: RemoteTrackPublication, participant: RemoteParticipant) => {
        if (track.kind !== TrackKind.KIND_AUDIO) return;
        void captureTrack(meta, participant, track);
      },
    );

    await ctx.connect();
  },
});

// Register the worker under the reserved agent identity + the configured agent
// name (matching the server's dispatch target).
if (import.meta.main) {
  cli.runApp(
    new WorkerOptions({
      agent: import.meta.filename,
      agentName: config.agentName,
      identity: agentIdentity(config.agentName),
    }),
  );
}
