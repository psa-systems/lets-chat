import { expect, test } from 'bun:test';
import { clipHeaders, clipUrl, postClip } from '../src/callback.js';

test('clipUrl targets the LC-813 agent-clip route and trims trailing slashes', () => {
  expect(clipUrl('https://chat.example.com', 42)).toBe(
    'https://chat.example.com/call/transcript/42/agent-clip',
  );
  expect(clipUrl('https://chat.example.com/', 7)).toBe(
    'https://chat.example.com/call/transcript/7/agent-clip',
  );
});

test('clipHeaders carry the bearer token, speaker id, and duration', () => {
  const h = clipHeaders('s3cr3t', 'user-9', 'audio/wav', 4.2);
  expect(h.authorization).toBe('Bearer s3cr3t');
  expect(h['x-speaker-id']).toBe('user-9');
  expect(h['content-type']).toBe('audio/wav');
  expect(h['x-duration-secs']).toBe('4.2');
});

test('postClip POSTs to the right URL with the right headers and returns the status', async () => {
  let seen: { url: string; init: RequestInit } | null = null;
  const fakeFetch = (async (url: string | URL | Request, init?: RequestInit) => {
    seen = { url: String(url), init: init ?? {} };
    return new Response(null, { status: 200 });
  }) as unknown as typeof fetch;

  const status = await postClip({
    baseUrl: 'https://chat.example.com',
    transcriptId: 5,
    speakerId: 'user-1',
    token: 'tok',
    body: new Uint8Array([1, 2, 3]),
    durationSecs: 5,
    fetchImpl: fakeFetch,
  });

  expect(status).toBe(200);
  expect(seen!.url).toBe('https://chat.example.com/call/transcript/5/agent-clip');
  expect(seen!.init.method).toBe('POST');
  const headers = seen!.init.headers as Record<string, string>;
  expect(headers.authorization).toBe('Bearer tok');
  expect(headers['x-speaker-id']).toBe('user-1');
});
