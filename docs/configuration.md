# Configuration

Every environment variable the server and desktop binaries read, and where its
value comes from. `.env.standalone` is the annotated template to copy for a
deployment; the values here are what the code actually reads.

Only the four `LETS_CHAT_BUNYIP_SSO_*` variables are mandatory. Everything else
either has a default or gates an optional feature that stays off until it is
set.

## Core

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | `/data` | Directory for SQLite `.db` files |
| `BIND_ADDR` | `0.0.0.0:8080` | Server listen address |
| `RUST_LOG` | `lets_chat=info` | Tracing filter |
| `LETS_CHAT_BASE_URL` | `http://localhost:8080` | Externally-reachable base URL. Used to build deep links in outbound mail (digest, mention/DM notifications) and the SSO redirect. |
| `LETS_CHAT_ENVIRONMENT` | (unset = production) | LC-864. Deployment label shown in the shell badge and on the sign-in, first-entry and invitation surfaces, so a dogfooding user can tell which deployment they are on. Empty, `production` or `prod` (any case) shows nothing; any other value (`staging`, `dev`, ...) is displayed verbatim. Declared by configuration, never inferred from the hostname. |
| `LETS_CHAT_SECRET_KEY` | (none) | Encrypts at-rest secrets (Web Push VAPID private key; the sealed IMAP password for email ingress). See [`LETS_CHAT_SECRET_KEY`](#lets_chat_secret_key) below. |

## Authentication

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_BUNYIP_SSO_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` / `_REDIRECT_URI` | (none) | Bunyip OIDC SSO, the sole sign-in path (LC-22). All four are **mandatory**; the server refuses to start without them. `_REDIRECT_URI` is `{base}/auth/bunyip/callback` and must match a `redirect_uris` row on the OP client. Discovery + JWKS are fetched at startup and must succeed. |
| `LETS_CHAT_BUNYIP_SSO_INSECURE_TLS` | (unset = strict) | DEV ONLY. `1`/`true` makes the SSO HTTP client accept invalid TLS certs (for a dev OP behind a self-signed cert). Never set in production: it disables issuer TLS authentication. |
| `SETUP_DEFAULT_ADMIN` | (unset) | DEV ONLY (DEV-300). `email:password`, the fleet-wide bootstrap-admin variable (menkent, bunyip, eform read the same name). While the deployment has **no** admin, a debug build seeds one unlinked admin row carrying that verified email; the first sign-in with the same email at the OP adopts it, so a developer does not have to be the first user to get an admin. The password half is accepted for format compatibility and never stored: there is no local password auth (LC-22). A **release** build refuses to seed and logs an error instead, so the variable cannot mint an admin on a real deployment. Because the OP owns the role on every login (LC-413), the OP must also claim `bunyip_role: admin` for that identity or the adopted account is demoted back to `user`; `dev/mock-oidc.py` claims it. |
| `LETS_CHAT_DEV_NO_SSO` | (unset) | DEV ONLY (LC-826). `1`/`true`/`yes` boots with **no** Bunyip client at all so the local smoke (`just verify`) can run without an identity provider: nobody can sign in and `/auth/bunyip/*` answer with a "not configured" login error. Logged as a warning at startup. Never set on a real deployment. |
| `IP2LOCATION_DB_PATH` | (none = disabled) | LC-580. Path to an offline IP2Location LITE DB11 `.BIN`. When set (and readable), a login from a new country emails a "new sign-in" alert; also the country signal for the LC-587 approval gate below. Unset or unreadable = the feature is silently disabled (one startup log line, login unaffected). |
| `LOGIN_APPROVAL_ENABLED` | (unset = off) | LC-587. Set to `1`/`true`/`yes` to gate suspicious logins with notify-and-approve: a login at the Bunyip callback from a new country (needs `IP2LOCATION_DB_PATH`) and/or a new device is withheld, a single-use 6-digit code is emailed, and the user re-submits it to finish (never a lock). Off = the LC-580 alert-only behavior. Opt-in per deployment; it can withhold a login, so it ships off. |

## Mail

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_SMTP_HOST` / `LETS_CHAT_SMTP_PORT` / `LETS_CHAT_SMTP_TLS` / `LETS_CHAT_SMTP_FROM` / `LETS_CHAT_SMTP_USERNAME` / `LETS_CHAT_SMTP_PASSWORD` | (none) | SMTP relay configuration. The four non-credential vars must be set together to enable outbound mail; `_TLS` is one of `tls` / `starttls` / `none`. Username+password are an optional pair. |

Digest cadence and opt-in are in [email-digests.md](email-digests.md); inbound
mail is in [email-ingress.md](email-ingress.md).

## Calls, stages, and transcription

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_ICE_SERVERS` | `[{"urls":"stun:stun.l.google.com:19302"}]` | JSON array of `RTCIceServer` objects for WebRTC calls and voice channels. Add a TURN entry for reliable NAT traversal. |
| `LETS_CHAT_LIVEKIT_URL` / `_API_KEY` / `_API_SECRET` | (none) | LC-512. Self-hosted LiveKit SFU carrying stage audio, which the browser mesh cannot scale past a handful of peers. All three are required; with any of them unset the stage control plane (roles, request-to-speak) still works and only the media is absent. `_URL` is the browser-facing signaling URL (`wss://...`); the server mints short-lived HS256 access tokens from the roster and never touches media. |
| `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN` / `_NAME` | (none) | LC-810. Shared secret (and agent identity) for the LiveKit transcription agent that captures stage audio server-side. Dispatch stays disabled unless LiveKit is configured **and** the token is set. |
| `LETS_CHAT_STT_URL` / `_API_KEY` / `_MODEL` / `_PROMPT` / `_PROVIDER` / `_TIMEOUT_SECS` / `_SCOPE` / `_WORKERS` / `_RATE_GLOBAL` / `_RATE_ROOM` | (none) | Optional **server-side** call transcription. Point `_URL` at a transcription endpoint to transcribe calls server-side instead of with the in-browser Web Speech engine: browser-agnostic (Firefox/Safari). Whether audio leaves the deployment depends on where `_URL` points: a self-hosted `openai`-shaped endpoint (whisper.cpp, faster-whisper, LocalAI, ...) keeps audio local, while pointing `_URL` at `https://api.openai.com` or `_PROVIDER=deepgram` at `https://api.deepgram.com/v1/listen` sends the audio to that third-party cloud. `_PROVIDER` (LC-593) selects the wire shape: `openai` (default) for the OpenAI-compatible `/v1/audio/transcriptions`, or `deepgram` for Deepgram's prerecorded `/v1/listen` (point `_URL` at `https://api.deepgram.com/v1/listen` and `_MODEL` at e.g. `nova-2`). `_API_KEY` is an optional token (Bearer for openai, `Token` scheme for deepgram); `_MODEL` defaults to `whisper-1`. `_PROMPT` (LC-591) is an optional glossary/style hint to bias spelling of names and jargon (OpenAI only; deepgram ignores it). The server requests real caption timestamps (`verbose_json` for openai, word timings for deepgram) and sends the speaker's preferred locale as a `language` hint when set; engines that ignore either degrade cleanly. `_TIMEOUT_SECS` (LC-590, default 60) replaces the 10s shared outbound-HTTP timeout for STT only and is scaled up by the recorded clip length, capped at 300s; transient failures (connect, timeout, 5xx, 429) are retried up to 3 times with backoff, a 4xx never is. A voice message that still fails is marked failed and shows its author a Retry control; a failed call clip surfaces a caption warning instead of looking like silence. `_SCOPE` (LC-592, default `both`) selects which stored attachments are transcribed - `both`, `voice` (voice notes only), `clips` (video clips only, the expensive path), or `none`; call captions are unaffected either way. `_WORKERS` (default 2) caps concurrent transcriptions, protecting a CPU-bound self-hosted engine, while `_RATE_GLOBAL` (default 30/min) and `_RATE_ROOM` (default 10/min) cap submissions per minute to bound the bill on a metered one. Over-cap voice messages are marked failed and offer their author a Retry; over-cap call clips are shed rather than queued, since a late caption is worthless and the next clip is 5 seconds away. Unset `_URL` = the in-browser engine. The endpoint is operator-trusted and **not** SSRF-filtered, so it may be `localhost`/internal; never point it at an untrusted host. |
| `LETS_CHAT_STT_LANGUAGE` | (unset = autodetect) | LC-859. ISO-639-1 fallback language for clips with no per-speaker locale, so the engine skips its per-clip detection pass. A per-speaker locale still wins. |
| `LETS_CHAT_STT_VAD_FILTER` | (unset = off) | LC-844. `1`/`true` asks the engine to run voice-activity detection, which suppresses transcripts hallucinated from silence. |
| `LETS_CHAT_STT_MIN_LOGPROB` / `_MAX_NO_SPEECH` | (built-in defaults) | LC-846 confidence gate: segments below the average-logprob floor, or above the no-speech probability ceiling, are dropped. An unparseable value falls back to the default rather than disabling the gate. |

## Optional AI surfaces

Configuring any of these only makes the capability **available**. LC-679 keeps
the whole AI surface off until an admin flips "AI features" in Admin > Settings,
and even then it is exposed only to site admins, enclave owners/admins and room
moderators. Every endpoint is operator-trusted and **not** SSRF-filtered, so it
may be `localhost` or internal; never point one at an untrusted host.

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_LLM_URL` / `_API_KEY` / `_MODEL` | (none) | AI **summaries** for saved call transcripts. Point `_URL` at an OpenAI-compatible `/v1/chat/completions` endpoint (Ollama, llama.cpp server, vLLM, LocalAI, ...) to show a "Summarize" action (summary + action items, cached) on the transcript page. `_API_KEY` is an optional bearer token; `_MODEL` defaults to `gpt-4o-mini`. Unset = the action is hidden. |
| `LETS_CHAT_EMBEDDINGS_URL` / `_API_KEY` / `_MODEL` | (none) | LC-549. Semantic search and "Find related" messages, a separate service from the chat LLM. Point `_URL` at an OpenAI-compatible `/v1/embeddings` endpoint and pull an embedding model. `_MODEL` defaults to `text-embedding-3-small`; set it to the model you pulled. Unset = "Find related" is hidden and semantic search degrades to keyword (FTS). |
| `LETS_CHAT_VISION_URL` / `_API_KEY` / `_MODEL` | (none) | LC-667. Image alt-text auto-draft. Needs a multimodal model behind an OpenAI-vision-compatible endpoint; `_MODEL` defaults to `llava`. The image is sent only to this endpoint. |
| `LETS_CHAT_AI_MODERATION` | (unset = off) | LC-670. Truthy **and** an LLM configured: posted messages are classified in the background and clear spam / harassment / inappropriate cases are flagged to the admin report queue for human review, never auto-deleted. |
| `LETS_CHAT_WEEKLY_RECAP` | (unset = off) | LC-671. Truthy **and** an LLM configured: each active user is DMed a short weekly recap from the assistant bot. Quiet weeks are skipped. |
| `LETS_CHAT_GIPHY_API_KEY` / `_RATING` | (none) | Composer GIF picker. The server proxies search to Giphy and re-hosts the chosen GIF same-origin, never hotlinked into messages. `_RATING` is `g` / `pg` / `pg-13` / `r`, default `pg-13`. Unset key = the GIF button is hidden. |

## Notifications and retention

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_PUSH_CONTACT` | `mailto:admin@localhost` | VAPID contact address sent with Web Push delivery requests. |
| `LETS_CHAT_RETENTION_SWEEP_ENABLED` | (unset = disabled) | Set to `1` or `true` to enable the destructive hard-delete sweep that enforces per-room message `retention_days`. Read once at startup; flipping it requires a restart. |
| `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED` | (unset = enabled) | LC-78-AVATAR-PROXY. When enabled (the default), a bridge daemon may submit a `foreign_avatar` URL: the server fetches it once, magic-byte-sniffs and re-encodes it through the uploads pipeline, and serves it same-origin. Set to `false`/`0`/`no`/`off` to restore v1's posture (reject any non-null `foreign_avatar` with HTTP 400). Read per request. See [protocol-bridges.md](protocol-bridges.md). |

## Desktop app

These variables are read by the desktop binary on the user's machine, never by the server, so they are deliberately absent from `.env.standalone` and `.env.saas` (those are server deployment templates) and no `.env.desktop` exists. Set them in the desktop process environment.

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_SERVER_URL` | `http://localhost:8080` | URL the desktop wrapper opens. Server-only deployments can ignore it. |
| `LETS_CHAT_UPDATE_REGISTRY_URL` / `_REPOSITORY` / `_TAG` | `https://dev.a8n.run` / `psa-systems-private/lets-chat` / `latest-{platform}` | OCI registry the desktop self-updater pulls its release artifact from (LC-733). The updater fetches `{registry}/v2/{repository}/manifests/{tag}` and the single artifact blob it names, authenticated as the signed-in user. The release build compiles the same two values in, derived from the registry and repository it publishes the artifacts to (LC-831); the defaults shown are the fallback for a build with no injection, and a value set here still wins at run time. Self-hosters should point this at their own registry: the shipped default serves membership-gated binaries and is not anonymously readable. Every fetch and each redirect hop is validated against a public-IP SSRF filter. |
| `LETS_CHAT_UPDATE_TOKEN` | (unset) | Bearer for the update registry, overriding the credential the app receives from the server after a Bunyip sign-in. For a headless `--check-update` / `--update` with no GUI session. |
| `LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE` | (unset = off) | Exempts only the initial registry URL from the SSRF filter (private internal mirror or loopback test fixture). Redirect targets are still validated. |

## `LETS_CHAT_SECRET_KEY`

Encrypts at-rest secrets used by features that store sensitive data: Web Push (VAPID private key) and the sealed IMAP password for email ingress. Future encrypted-at-rest features will reuse the same key. (SMTP credentials and the Bunyip SSO client secret are passed via environment variables, not the database, so they do not depend on this key.)

**Format.** Any non-empty string. The server SHA-256-hashes it to derive a 32-byte AES-256-GCM key, so length and encoding don't matter; entropy does. Use at least 32 random bytes.

**Generate one:**

```sh
head -c 32 /dev/urandom | base64
```

(or `openssl rand -base64 32` if OpenSSL is handy.)

**Without it.** Web Push and encrypted email ingress are silently disabled. Settings shows the relevant checkboxes as disabled with help text pointing back here. The HTTP API is also unavailable: see [api.md](api.md) for the 401 behavior this causes for all token auth.

**If you lose it.** Encrypted rows become undecryptable, but the rest of the app continues to run.

- *Web Push:* existing browser subscriptions become orphaned (the server can no longer sign messages for them). Users re-subscribe automatically on their next @-mention or DM after a fresh keypair is generated.
- *Email ingress:* the sealed IMAP password fails to decrypt and the poll loop stays disabled until the IMAP config is re-entered under the new key.

**If you rotate it.** The app does NOT auto-regenerate encrypted rows. On startup with a new key:

- *Web Push:* the VAPID keypair fails to decrypt and a `vapid keypair load failed` warning is logged. Push stays disabled until the row is cleared and a fresh keypair generated:
  ```sh
  sqlite3 /data/settings.db "DELETE FROM vapid_keypair;"
  ```
  After restart, browser subscriptions issued under the old keypair are invalid; users may need to clear site data or unregister the service worker before a new subscription takes hold.
- *Email ingress:* re-enter the IMAP config from `/admin/settings` so the password is re-sealed under the new key.

**Storage.** Treat it like a database password. Use Docker `--env-file`, your deployment's secret manager, or a `.env` file with restricted permissions. Don't bake it into a committed `compose.yml`.
