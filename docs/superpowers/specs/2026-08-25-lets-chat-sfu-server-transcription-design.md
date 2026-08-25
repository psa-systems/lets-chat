# Server-Side Per-Participant Call Transcription (SFU) - Design

Date: 2026-08-25
Status: Approved design, pending implementation plan. Acquisition mechanism decided (sidecar LiveKit Agent).
Tracking: LC-810 (follow-on from LC-765; the interim honesty notice shipped in PR #764).

## Problem

Transcription today is per-client local capture. Each browser captures its OWN microphone and either transcribes locally (Web Speech) or POSTs ~5s clips to `POST /call/transcript/{id}/audio`, where the server forwards them to an external STT endpoint. A segment's speaker is simply the authenticated poster, because each browser only ever sends its own mic. A transcript is therefore complete only if every participant independently turns on Transcribe in a supported browser. The interim notice (LC-765) tells users this; LC-810 is the real fix.

The goal: **starting transcription once on an SFU (huddle) call produces a transcript of the whole call, speakers attributed, without every participant enabling it themselves.** The 1:1 mesh path (DM calls) keeps its existing client-side capture unchanged.

## Current state (verified against source, 2026-08-25)

The downstream is already built and reusable; only the upstream (observing remote audio) is missing.

- **Server LiveKit footprint is JWT minting only.** `server/src/livekit.rs` mints HS256 access tokens (`mint_token`, `livekit.rs:133`) and nothing else: no room-service client, no egress client, no webhook receiver, no Rust media SDK. The module doc says so outright. The only LiveKit crate is `jsonwebtoken = "9"`; there is no `livekit` / `livekit-api` / `webrtc-sys` anywhere.
- **STT sink is a clean clip-in / text-out abstraction.** `SttClient::transcribe(SttRequest) -> SttResult` (`stt.rs:291`); `SttRequest { audio: Vec<u8>, content_type, language, duration_secs, timeout_secs }` (`stt.rs:226`); `SttResult { text, segments }` (`stt.rs:166`). Production goes through `transcribe_with_retry` (`stt.rs:300`, backoff `[1s, 4s]`). Provider abstraction (OpenAI / Deepgram), model, glossary prompt, and timeouts are all config (`SttConfig::from_env`, `stt.rs:125`).
- **Load control already exists.** `stt_load::permits()` (process-wide `Semaphore`, size `LETS_CHAT_STT_WORKERS`, default 2) and `stt_load::try_admit(limits, room_id)` (fixed-window global + per-room caps, `LETS_CHAT_STT_RATE_GLOBAL`/`_RATE_ROOM`) at `stt_load.rs:128`/`:142`.
- **Storage + broadcast are id-agnostic.** `db::transcripts::append_segment(pool, transcript_id, user_id, text, raw_text, duration_ms)` (`db/transcripts.rs:149`) inserts into `transcript_segments (transcript_id, user_id, text, raw_text, duration_ms, spoken_at)` (`0066_call_transcripts.sql`, no FK on user ids since they live in a separate auth pool). `record_and_broadcast` (`transcripts.rs:195`) appends then fans out `ChatEvent::TranscriptSegment { speaker_id, speaker_name, text, ... }` per recipient. None of this assumes the speaker is the poster; it just takes a user id.
- **Attribution is free.** A LiveKit participant's identity IS the lets-chat user id (`mint_token` sets `sub = &user.id`; `huddle_sfu.js:97` keys tiles on `participant.identity` with that exact comment).
- **SFU vs mesh is a global config flag, not per-call negotiation.** `routes/call.rs:31` emits `"huddleSfu": livekit::available()`: when LiveKit is configured, huddles are entirely SFU; when not, entirely mesh. DM calls are always mesh (`get_huddle_token` refuses `room_type == "dm"`, `stage.rs:82`). There is no mesh<->SFU handover.

**The one thing the current ingest can't do:** `POST /call/transcript/{id}/audio` derives the speaker from `AuthUser` (`transcripts.rs:355`). Server-side capture produces clips attributed to *arbitrary* participants, so it needs a *trusted* ingest that carries an explicit `speaker_id` rather than inferring it from the caller.

## Decision: sidecar LiveKit Agent (dumb media bridge)

A separate, small service - built on LiveKit's official Agents SDK (Python or Node) - joins the SFU room, subscribes to each participant's audio track, frames each track into ~5s clips, and POSTs each clip **back into lets-chat's existing STT pipeline** tagged with the speaker's identity. The agent runs **no STT of its own**: it is a pure audio-acquisition bridge, so all STT policy (provider, model, glossary, rate limits, retry, storage, broadcast) stays in lets-chat, configured once.

### Why this mechanism

- **Keeps native/media weight out of the Rust monolith.** No `webrtc-sys`/libwebrtc dependency, no C++ toolchain in the builder image, no build-time or maintenance blow-up. The monolith stays lean, matching its "optional external service via env" posture (LLM, STT, GIF, LiveKit are all already this shape).
- **Reuses the entire downstream unchanged.** Because the agent posts audio clips (not text) back to lets-chat, `SttClient`, `stt_load` permits/caps, `append_segment`, and `TranscriptSegment` broadcast are all reused verbatim. One STT config, one glossary, one rate limiter, one storage path.
- **Uses LiveKit's blessed real-time-media path.** The Agents framework exists precisely to join a room and process participant tracks; it handles subscribe-on-publish, track lifecycle, and reconnection for us.

### Alternatives rejected

- **LiveKit Egress.** Heaviest operator infra (a Chrome/ffmpeg egress service + redis) and would put a room-service/egress client into the Rust server. Rejected for operator burden.
- **In-process Rust bot (`livekit` crate).** Pulls `webrtc-sys`/libwebrtc into the monolith: much slower/larger builds, a new native toolchain in the builder image, ongoing maintenance. Rejected as it fights the lean posture.
- **Agent runs STT and posts text.** Would split STT config/policy across two services (two provider configs, two glossaries, a second rate limiter) and bypass `stt_load`. Rejected in favor of the dumb-bridge (post-audio) shape so STT stays single-sourced in lets-chat.

## Architecture

```
                 (1) start transcription on an SFU call
Browser  ─────────────────────────────────────────────►  lets-chat server
                                                             │
                                    (2) AgentDispatch(room, job metadata) via LiveKit API
                                                             ▼
                                                        LiveKit SFU  ──► dispatches job
                                                             │
                                                             ▼
                                                     Transcription Agent (sidecar)
                                                       joins room as a hidden participant,
                                                       subscribes to every remote audio track
                                                             │
                                       (3) per track: frame ~5s clips, encode to ogg/wav
                                                             │
                          POST /call/transcript/{id}/agent-clip  (bearer agent token)
                          headers: X-Speaker-Id, X-Content-Type, X-Duration-Secs, X-Language
                                                             ▼
                                                        lets-chat server
                                        (4) auth + validate → stt_load admit/permit →
                                            transcribe_with_retry → append_segment(speaker_id) →
                                            broadcast TranscriptSegment  (existing pipeline)
```

### Control flow (lifecycle)

1. A participant starts transcription on a huddle (SFU) call. lets-chat creates/uses the `call_transcripts` row (status `active`) exactly as today.
2. If the call is SFU (`livekit::available()` and it's a huddle, not a DM) **and** the agent is configured, lets-chat issues an **explicit LiveKit agent dispatch** into the room, with job metadata `{ transcript_id, room_id, callback_base_url }`. This is a single signed API call to LiveKit (twirp/REST) using the api key/secret the server already holds - not a media SDK. It is the only genuinely new server-side LiveKit surface.
3. The agent picks up the job, joins the room as a hidden, non-publishing participant (`can_publish: false`, and hidden from tiles - see Visibility below), subscribes to each remote audio track (and to any track published later), frames each into ~5s clips mirroring the browser `CLIP_MS` cadence, encodes to a container STT accepts (ogg/opus or wav), and POSTs each clip to the callback.
4. lets-chat authenticates the callback, resolves the speaker, and runs the clip through the **existing** `stt_load` → `transcribe_with_retry` → `append_segment` → broadcast path.
5. On stop-transcription (or when the room empties), lets-chat deletes the dispatch / signals the agent to leave; the agent disconnects.

### Visibility and identity

- The agent joins with a reserved identity (e.g. `agent:transcriber`) that is **not** a lets-chat user id, so it can never be mistaken for a speaker. Browser tile code already keys on `participant.identity`; the agent identity is filtered out of the roster/tiles (one small allow-list check in `huddle_sfu.js`).
- Speaker attribution uses each *remote* track's participant identity, which equals the lets-chat user id. The callback carries it as `X-Speaker-Id`.

## New trusted ingest endpoint

`POST /call/transcript/{id}/agent-clip`

- **Auth:** `Authorization: Bearer <LETS_CHAT_TRANSCRIBE_AGENT_TOKEN>`, compared in constant time. No `AuthUser`; this is a service-to-service route. The route is exempt from the normal session-auth middleware and gated solely on the agent token.
- **Headers:** `X-Speaker-Id` (LiveKit identity = user id), `X-Content-Type` (e.g. `audio/ogg`), `X-Duration-Secs`, optional `X-Language`.
- **Body:** raw audio bytes for one clip.
- **Validation (defense in depth against a compromised agent forging speakers):**
  1. Agent token valid (constant-time).
  2. `transcript_id` exists and is `active`.
  3. `X-Speaker-Id` is a **current participant of that transcript's room** (checked against the live roster / membership), else 403. This bounds the agent to real speakers of the specific room.
  4. Body non-empty; content-type on an allow-list.
- **Processing:** identical to the browser path - `stt_load::try_admit(room_id)` (429 on refuse), `stt_load::permits().try_acquire()` (shed → 429), `transcribe_with_retry` at `LIVE_CLIP_TIMEOUT_SECS`, then `record_and_broadcast(state, room, transcript_id, speaker_user, text, duration_ms)` with the **resolved speaker** (not the caller).

### Mechanism-independent refactor (first code slice)

Extract the shared core of the existing `audio` handler (`transcripts.rs:286`) - admit → permit → transcribe → `record_and_broadcast` - into a private helper `ingest_clip(state, room, transcript_id, speaker: &User, audio, content_type, language, duration)`. The current browser route calls it with `speaker = self`; the new agent route calls it with the resolved speaker. This is fully unit-testable today with a fake `SttClient`, with no LiveKit or agent present, and is the natural first implementation PR.

## Reuse map

| Concern | Source | Change |
| --- | --- | --- |
| STT call + retry | `stt.rs` (`transcribe_with_retry`) | none |
| Provider/model/glossary/timeout config | `SttConfig::from_env` | none |
| Rate limit + worker permits | `stt_load.rs` | none (per-room caps now cover agent clips too) |
| Segment storage | `db::transcripts::append_segment` | none (already takes an arbitrary `user_id`) |
| Live broadcast | `record_and_broadcast` / `ChatEvent::TranscriptSegment` | none |
| Speaker → name lookup | user store | reused; agent path resolves name from `speaker_id` |
| Ingest handler core | `routes/transcripts.rs::audio` | refactor into `ingest_clip` (see above) |
| Trusted ingest route | new | `POST /call/transcript/{id}/agent-clip` |
| Agent dispatch on start/stop | new | signed LiveKit dispatch API call, gated on config |
| The agent service | new (separate deployable) | Agents SDK; no STT of its own |

## Config surface (operator-facing)

New env, all optional; the feature is inert unless set:

- `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN` - shared secret authenticating the agent callback. Presence of this (plus `livekit::available()`) is the gate: no token ⇒ no dispatch, and server-side capture is off; the mesh path and the LC-765 notice remain.
- Agent-side env (its own process): `LIVEKIT_URL` / `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` (same values lets-chat uses), `LETS_CHAT_BASE_URL` (callback target), `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN` (same secret). The agent needs **no** STT config in the dumb-bridge design.

`livekit::available()` stays the SFU gate. A helper `transcribe_agent_available()` = `livekit::available() && token present` gates the dispatch and the new route's activation.

## Correctness requirements (maps to LC-810 ACs)

1. **Whole-call transcript.** With transcription enabled once on a 3-participant SFU call, the agent subscribes to all three tracks and posts clips for each; the transcript contains all three speakers. (End-to-end, staging.)
2. **Attribution, incl. leave/rejoin.** Each segment is attributed via the track's participant identity. A participant leaving and rejoining is a new subscription for the same identity, so attribution is stable. (Agent test + staging.)
3. **Mute contributes nothing (LC-626).** In LiveKit, muting unpublishes the audio track, so the agent has no track to subscribe to and posts nothing. The invariant is enforced at the agent (track absence) and asserted by an **agent-side test** (muted participant ⇒ zero clips). lets-chat additionally never fabricates a segment without a clip.
4. **Mid-call joiner captured from join.** The agent subscribes on `TrackPublished`, so a joiner is captured from the moment their mic publishes. (Agent test + staging.)
5. **Mesh/DM unchanged.** When the call is mesh (LiveKit unset) or a DM, no dispatch happens; the browser local-capture path and the LC-765 notice are untouched. (Existing tests + a guard test that the agent route is inert when `transcribe_agent_available()` is false.)

## Security notes

- The `agent-clip` route accepts an arbitrary `speaker_id`, so it MUST be strongly authenticated (constant-time secret) and MUST validate the speaker is a live participant of that specific transcript's room (validation step 3). Without step 3, a leaked token could forge words from any user in any room.
- The callback is service-to-service over the operator's network; the token is the trust boundary. Rotating it is a config change + agent restart.
- Reusing `stt_load` means the agent cannot exhaust STT capacity beyond the existing global/per-room caps; a busy call sheds clips (429) exactly like the browser path.
- No new inbound audio is stored beyond the existing `transcript_segments` text; raw clips are transient (transcribed then dropped), same as today.

## Staged rollout (each stage its own PR/ticket)

1. **lets-chat: ingest refactor + trusted route.** Extract `ingest_clip`; add `POST /call/transcript/{id}/agent-clip` with token auth + participant validation; unit tests with a fake `SttClient` (no LiveKit needed). Mechanism-independent; ships dark behind config.
2. **lets-chat: dispatch wiring.** On start/stop transcription for an SFU huddle, dispatch/stop the agent via the signed LiveKit API; add `transcribe_agent_available()` gate; filter the agent identity from rosters/tiles. Config + tests.
3. **Sidecar agent service.** New deployable (Agents SDK): join on dispatch, subscribe tracks, frame + encode ~5s clips, POST back; handle mute (no track), mid-call join (`TrackPublished`), leave/rejoin, backpressure on 429. Agent tests for ACs 2-4.
4. **Ops.** Compose service definition, operator docs (`docs/deploy-runbook.md`), env reference, secret rotation note.
5. **End-to-end QA on staging** with a real LiveKit deployment across the five ACs, then flip on.

## Open questions for the implementation plan

- **Agent SDK language.** LiveKit Agents is most mature in Python; Node is an option. Pick per operator/runtime preference; it does not affect the lets-chat contract above.
- **Clip container/codec** the agent encodes to (ogg/opus vs wav) vs what the configured STT provider ingests best; confirm against OpenAI/Deepgram accepted formats during stage 3.
- **Dispatch API surface** (explicit `AgentDispatch` create/delete vs an auto-dispatch rule keyed on room-name prefix `huddle-`). Explicit dispatch is preferred so lets-chat controls start/stop precisely; confirm the exact API/version at stage 2.
- **Barge-in / overlapping speakers**: per-track clips are independent, so overlapping speech yields interleaved segments (correct). Confirm ordering/rendering in the drawer is acceptable at QA.
