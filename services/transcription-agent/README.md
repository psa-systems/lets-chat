# Transcription agent (LC-810 stage 3)

A sidecar [LiveKit Agents](https://docs.livekit.io/agents/) worker that gives
lets-chat **server-side, per-participant** call transcription. It is the piece
lets-chat dispatches (LC-814) when someone starts transcription on an SFU huddle.

It is a **dumb media bridge**: it joins the LiveKit room, subscribes to each
participant's audio track, slices each stream into ~5s clips, and POSTs them to
lets-chat's trusted `agent-clip` route (LC-813). It runs **no STT of its own** -
all speech-to-text policy (provider, model, glossary, rate limits, storage, live
broadcast) stays in lets-chat, configured once.

## How it fits

```
lets-chat (start transcription on an SFU huddle)
   └─ dispatch (LC-814) ──► LiveKit ──► THIS AGENT joins as `agent-<name>`
                                          subscribes to each audio track
                                          frames ~5s WAV clips per participant
        POST /call/transcript/{id}/agent-clip  ◄──────────┘
        (bearer token, X-Speaker-Id = participant identity)
   └─ lets-chat STT pipeline transcribes + stores + broadcasts the caption
```

The agent never renders in the call UI: it joins under a reserved `agent-`
identity that `huddle_sfu.js` filters out of the roster.

## Configuration (environment)

| Var | Required | Default | Meaning |
| --- | --- | --- | --- |
| `LIVEKIT_URL` | yes | | LiveKit signaling URL (same value lets-chat uses). |
| `LIVEKIT_API_KEY` | yes | | LiveKit API key. |
| `LIVEKIT_API_SECRET` | yes | | LiveKit API secret. |
| `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN` | yes | | Shared bearer for the `agent-clip` callback. MUST equal the value set on lets-chat (LC-813). |
| `LETS_CHAT_TRANSCRIBE_AGENT_NAME` | no | `transcriber` | Registered agent name. MUST equal lets-chat's `LETS_CHAT_TRANSCRIBE_AGENT_NAME` so dispatch targets this worker. |
| `CLIP_MS` | no | `5000` | Clip cadence, mirroring the browser MediaRecorder. |

The transcript id and the lets-chat callback base URL are **not** env: they
arrive in each dispatch's job metadata (`{transcript_id, base_url}`, sent by
LC-814), so one worker serves every room.

## Run

```
bun install
bun run start      # production worker (waits for dispatch)
bun run dev        # local dev worker
bun test           # unit tests (no LiveKit needed)
```

Or via Docker:

```
docker build -t lets-chat-transcription-agent .
docker run --rm \
  -e LIVEKIT_URL=... -e LIVEKIT_API_KEY=... -e LIVEKIT_API_SECRET=... \
  -e LETS_CHAT_TRANSCRIBE_AGENT_TOKEN=... \
  lets-chat-transcription-agent
```

## What is tested here vs at staging

Unit-tested (`bun test`, no LiveKit): WAV encoding, the per-participant clipper
(cadence, attribution, tail flush, and the muted-participant-no-clip invariant),
the callback URL/headers/POST, and config + job-metadata parsing.

The LiveKit round-trip (connect, subscribe, dispatch, the audio-frame format) is
verified at **staging** (LC-810 stage 5) against a live LiveKit deployment, since
it cannot be exercised without one.

## Design

See `docs/superpowers/specs/2026-08-25-lets-chat-sfu-server-transcription-design.md`
and the LC-810 stage tickets (LC-813 ingest, LC-814 dispatch, this = LC-815).
