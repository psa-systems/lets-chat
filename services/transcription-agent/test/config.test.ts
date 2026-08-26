import { expect, test } from 'bun:test';
import { loadConfig, parseJobMeta } from '../src/config.js';

const baseEnv = {
  LIVEKIT_URL: 'wss://lk.example.com',
  LIVEKIT_API_KEY: 'key',
  LIVEKIT_API_SECRET: 'secret',
  LETS_CHAT_TRANSCRIBE_AGENT_TOKEN: 'tok',
};

test('loadConfig applies defaults for the agent name and clip cadence', () => {
  const c = loadConfig(baseEnv);
  expect(c.agentName).toBe('transcriber');
  expect(c.clipMs).toBe(5000);
});

test('loadConfig honours overrides and ignores a bad CLIP_MS', () => {
  expect(loadConfig({ ...baseEnv, CLIP_MS: '3000' }).clipMs).toBe(3000);
  expect(loadConfig({ ...baseEnv, CLIP_MS: 'nope' }).clipMs).toBe(5000);
  expect(loadConfig({ ...baseEnv, LETS_CHAT_TRANSCRIBE_AGENT_NAME: 'scribe' }).agentName).toBe(
    'scribe',
  );
});

test('loadConfig throws when a required var is missing', () => {
  expect(() => loadConfig({ ...baseEnv, LETS_CHAT_TRANSCRIBE_AGENT_TOKEN: '' })).toThrow(
    /LETS_CHAT_TRANSCRIBE_AGENT_TOKEN/,
  );
});

test('parseJobMeta reads transcript_id and base_url', () => {
  const m = parseJobMeta('{"transcript_id": 12, "base_url": "https://chat.example.com"}');
  expect(m.transcriptId).toBe(12);
  expect(m.baseUrl).toBe('https://chat.example.com');
});

test('parseJobMeta rejects missing or invalid fields', () => {
  expect(() => parseJobMeta('not json')).toThrow(/valid JSON/);
  expect(() => parseJobMeta('{"base_url": "x"}')).toThrow(/transcript_id/);
  expect(() => parseJobMeta('{"transcript_id": 1}')).toThrow(/base_url/);
});
