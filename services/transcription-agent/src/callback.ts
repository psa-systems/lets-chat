// LC-815: post a captured clip back to lets-chat's trusted agent-clip ingest
// (LC-813). The agent runs no STT itself; it hands lets-chat the audio bytes
// attributed to the track's participant, and lets-chat's existing pipeline
// (rate caps, STT, storage, broadcast) does the rest.

/** The LC-813 trusted-ingest URL for one transcript. */
export function clipUrl(baseUrl: string, transcriptId: number): string {
  const base = baseUrl.trim().replace(/\/+$/, '');
  return `${base}/call/transcript/${transcriptId}/agent-clip`;
}

/**
 * Headers for one clip POST. The bearer token is the LC-813 trust boundary;
 * X-Speaker-Id is the track's LiveKit identity (== lets-chat user id), so
 * lets-chat attributes the segment to the participant, not the caller.
 */
export function clipHeaders(
  token: string,
  speakerId: string,
  contentType: string,
  durationSecs: number,
): Record<string, string> {
  return {
    authorization: `Bearer ${token}`,
    'content-type': contentType,
    'x-speaker-id': speakerId,
    'x-duration-secs': String(durationSecs),
  };
}

export interface PostClipArgs {
  baseUrl: string;
  transcriptId: number;
  speakerId: string;
  token: string;
  body: Uint8Array;
  contentType?: string;
  durationSecs: number;
  fetchImpl?: typeof fetch;
}

/**
 * POST one clip. Best-effort: returns the HTTP status. lets-chat sheds with 429
 * under load and rejects a non-participant with 403; the caller logs and moves
 * on to the next clip (a live caption is worthless replayed late).
 */
export async function postClip(args: PostClipArgs): Promise<number> {
  const contentType = args.contentType ?? 'audio/wav';
  const doFetch = args.fetchImpl ?? fetch;
  const res = await doFetch(clipUrl(args.baseUrl, args.transcriptId), {
    method: 'POST',
    headers: clipHeaders(args.token, args.speakerId, contentType, args.durationSecs),
    body: args.body,
  });
  return res.status;
}
