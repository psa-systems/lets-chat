// LC-815: agent configuration from the environment. The agent shares LiveKit
// credentials with lets-chat and the LC-813 callback token; the callback base
// URL and transcript id come from the dispatch job metadata (LC-814), not env,
// so one agent worker serves every room.

export interface JobMeta {
  transcriptId: number;
  baseUrl: string;
}

export interface AgentConfig {
  livekitUrl: string;
  livekitApiKey: string;
  livekitApiSecret: string;
  /** LC-813 bearer for the agent-clip callback. */
  callbackToken: string;
  /** Registered agent name; must match the server's LETS_CHAT_TRANSCRIBE_AGENT_NAME. */
  agentName: string;
  /** Clip cadence, mirroring the browser MediaRecorder. */
  clipMs: number;
}

const DEFAULT_AGENT_NAME = 'transcriber';
const DEFAULT_CLIP_MS = 5000;

function required(env: Record<string, string | undefined>, key: string): string {
  const v = env[key]?.trim();
  if (!v) throw new Error(`missing required env ${key}`);
  return v;
}

export function loadConfig(env: Record<string, string | undefined> = process.env): AgentConfig {
  const clipMs = Number(env.CLIP_MS?.trim() || DEFAULT_CLIP_MS);
  return {
    livekitUrl: required(env, 'LIVEKIT_URL'),
    livekitApiKey: required(env, 'LIVEKIT_API_KEY'),
    livekitApiSecret: required(env, 'LIVEKIT_API_SECRET'),
    callbackToken: required(env, 'LETS_CHAT_TRANSCRIBE_AGENT_TOKEN'),
    agentName:
      env.LETS_CHAT_TRANSCRIBE_AGENT_NAME?.trim() || DEFAULT_AGENT_NAME,
    clipMs: Number.isFinite(clipMs) && clipMs > 0 ? clipMs : DEFAULT_CLIP_MS,
  };
}

/** Parse and validate the dispatch job metadata (LC-814 sends this JSON). */
export function parseJobMeta(raw: string | undefined): JobMeta {
  let obj: unknown;
  try {
    obj = JSON.parse(raw || '{}');
  } catch {
    throw new Error('job metadata is not valid JSON');
  }
  const m = obj as Record<string, unknown>;
  const transcriptId = Number(m.transcript_id);
  const baseUrl = typeof m.base_url === 'string' ? m.base_url.trim() : '';
  if (!Number.isInteger(transcriptId) || transcriptId <= 0) {
    throw new Error('job metadata missing a valid transcript_id');
  }
  if (!baseUrl) throw new Error('job metadata missing base_url');
  return { transcriptId, baseUrl };
}
