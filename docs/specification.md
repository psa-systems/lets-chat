# Let's Chat Specification

## 1. Purpose and scope

Let's Chat is a self-hosted team chat application delivered as a single Rust server binary. It provides rooms grouped
into enclaves, direct messages, threads, voice and video calls, a scoped JSON HTTP API, incoming and outgoing webhooks,
bridges to other chat protocols, and an admin area. A desktop shell and a transcription sidecar are built from the same
repository.

Users: members of an installation (sign in through an external identity provider), room and enclave moderators,
installation admins, and programs using API tokens.

Not in scope (not implemented in code): local username/password registration and sign-in (the sign-in path is Bunyip
single sign-on only; `LETS_CHAT_DEV_NO_SSO` disables sign-in entirely for development smoke runs), multi-process
clustering (sessions, presence and the live-update hub are in process), and end-to-end message encryption.

## 2. Architecture

Cargo workspace members:

- `server` (package `lets-chat-server`): the web application. Library `lets_chat` plus two binaries selected by feature:
  `lets-chat` (feature `standalone`, the default) and `lets-chat-saas` (feature `saas`).
- `desktop` (package `lets-chat-desktop`): a Tauri 2 desktop shell. It probes the configured server URL, loads the
  server in a webview when reachable (otherwise a built-in welcome page), and provides a system tray, native
  notifications, a global shortcut that severs remote-control sessions, single-instance enforcement, and a
  self-updater that checks a SHA-256 from an update manifest before replacing its own executable.
- `ip-policy`: shared library exposing `is_globally_routable`, used by the server and the desktop shell for outbound
  address checks.

Other components: `services/transcription-agent` (a Bun/TypeScript LiveKit Agents worker, section 8) and
`ci-build/` (Dockerfiles and repository check scripts).

Server stack: tokio (multi-thread runtime, 32 worker threads), axum 0.8 with WebSocket and multipart support, askama
templates rendered server-side, htmx-style fragment updates delivered over the WebSocket, fluent-templates for
localization (`server/locales`), sqlx with SQLite, tower-http compression and static file serving, reqwest with rustls.
Static assets are served from `server/assets` under `/assets` (versioned `?v=` URLs get an immutable cache header).

Storage: three SQLite databases in the data directory, each with its own embedded migration set:

- `auth` (`server/migrations/auth`): users, sessions, profiles, API tokens, push subscriptions, login history.
- `chat` (`server/migrations/chat`): enclaves, rooms, messages, uploads, polls, webhooks, transcripts and related data.
- `settings` (`server/migrations/settings`): installation settings (IMAP inbox, Giphy, VAPID keypair, and similar).

Live updates: an in-process hub (`ws::hub::Hub`) fans out `ChatEvent` values to WebSocket connections; the server
renders each event to an HTML fragment per connection. Background tasks run as spawned tokio loops (section 3.12).

Router layering (outermost first): security headers, compression, request tracing, then public webhook and feed
routes, the `/api/v1` routes, and the session-based application routes behind these middlewares: user injection from the
session cookie, navigation tracking, locale resolution, branding CSS injection, first-entry handle gate, maintenance
mode gate.

## 3. Functional requirements

### 3.1 Authentication and accounts

- Sign-in is a Bunyip OpenID Connect relying-party flow with PKCE and nonce: `/auth/bunyip/start` stores a pending row
  and redirects, `/auth/bunyip/callback` consumes it, exchanges the code, verifies the EdDSA `id_token` against the
  provider's JWKS, and creates a session. The server refuses to start when the SSO configuration is invalid or the
  provider cannot be initialized, unless `LETS_CHAT_DEV_NO_SSO` is set.
- Sessions use the `session` cookie; the session token is stored hashed at rest. A Bunyip `access_token` cookie whose
  subject differs from the signed-in user triggers re-authentication (identity-swap defence).
- Optional suspicious-login approval gate (`LOGIN_APPROVAL_ENABLED`), login-location alerts using an IP2Location
  database, and per-user session listing and revocation.
- Two-factor authentication secrets are sealed with AES-256-GCM under a key derived from `LETS_CHAT_SECRET_KEY`. With
  the variable unset, two-factor authentication is disabled; a present but weak key aborts startup.
- Users have profile fields, status (active, idle, custom status with expiry), do-not-disturb schedule, blocks,
  notification keywords, locale, theme, palette, scale and density settings, and can export their data and delete
  their account.
- A user must confirm a handle on first entry (`enforce_handle_confirmed` gate).
- The first admin can be created for development through `SETUP_DEFAULT_ADMIN`.
- Roles: users have a role string (`user`, `admin`, and `bridge` for bridge accounts); enclave members have role
  `owner`, `admin` or `member` (one owner per enclave).

### 3.2 Enclaves and rooms

- An enclave groups rooms and members. Features: public discovery and join, invite codes, direct invitations
  (accept/decline), visibility, welcome message, branding (logo/icon), shared custom emoji, per-enclave message rate
  limit, member roles and trust levels, kick and ban/unban, ownership transfer, user groups, sidebar categories,
  storage quotas.
- Rooms are public or private, plus direct-message rooms (room type `dm`) and voice channels. Room features: name and
  description, wiki page, pins, posting policy, broadcast policy, slowmode, retention policy, per-room moderators
  (room RBAC), nicknames, notification preferences, files list, assistant, digest, stage mode, remote-control toggle,
  and automations.
- Per-user sidebar: starred rooms, collapsible categories, a quick switcher, an inbox, an activity page, saved items
  (bookmarks with labels), drafts, and unread tracking (read-all, per-message mark unread).

### 3.3 Messages

- Messages are Markdown rendered with syntax highlighting, LaTeX math rendered to MathML, mentions, channel links,
  emoji shortcodes and custom emoji. Maximum message length is 16000 characters.
- Operations: post, edit (with edit history), delete, permalink, raw view, forward to another room, report, pin,
  bookmark, quote-reply, translate, reactions, threads (replies, follow, mute, thread titles), required
  acknowledgements, scheduled messages (with repeat), reminders, polls and events, Giphy GIFs, voice messages,
  follow-up items, link previews (with a same-origin image proxy), and message expiry.
- Search covers full text (SQLite FTS) and saved searches; related-message suggestions use embeddings when an embeddings
  service is configured.
- Uploads: the default per-file limit is 10 MiB; call clips up to 64 MiB; custom emoji up to 256 KiB. Files are
  type-sniffed, images get thumbnails, and alt text can be set.

### 3.4 Calls

- One-to-one WebRTC calls signaled over the WebSocket, mesh voice channels, SFU huddles and stages through LiveKit,
  call reactions, screen sharing, a remote-control consent handshake, and voice diagnostics logged to an admin voice
  log.
- Call transcription: live captions through a configured speech-to-text service, per-participant server-side
  transcription via the transcription agent, transcript storage, export, AI summary and brief, and creation of
  follow-ups from a transcript.

### 3.5 Integrations surfaces

- Incoming webhooks (`/webhook/{secret}`, text up to 16 KiB), RSS and iCalendar feeds (`/feed/rss/{token}`,
  `/feed/ical/{token}`), outgoing webhooks with deliveries signed with an `X-LetsChat-Signature: sha256=<hmac>` header, and retry, custom slash commands, room feeds, email
  inboxes (inbound email polled from IMAP into rooms, with reply tokens), email digests and weekly recaps, bots, and
  protocol bridges (bridge accounts post through the API; see 4.3).
- Web push (VAPID), plus APNs and FCM registration endpoints.

### 3.6 AI-assisted features (all optional)

Enabled only when the matching service is configured: transcript and channel summaries, compose assist, reply
suggestions, per-room assistant, help-docs question answering, AI moderation triage, message embeddings and semantic
related-messages, image description (vision), and thread titles.

### 3.7 Moderation and administration

- Reports and a moderation queue, anti-spam rules, link filter with quarantine (approve/reject), moderation log, mute,
  ban, kick, per-user storage quotas, maintenance mode with a message (admins and probes stay reachable), backup and
  restore, installation branding, analytics with daily aggregates, support tickets with an escalation panel, feature
  flags, and bridge management.
- Backup produces a zip archive; a restore is staged and applied at the next start, before any database is opened. A
  failed pending restore stops the server from starting.

### 3.8 Retention

Per-room message retention with a preview, a sweep job (enabled by `LETS_CHAT_RETENTION_SWEEP_ENABLED`), and sweeper
status reporting; ephemeral (expiring) messages are removed by a one-minute sweeper.

### 3.9 Internationalization

Fluent catalogs in `server/locales`; locale resolved per request from user setting and request.

### 3.10 Data export

Users export their own data from settings; room and enclave exports are available through the export routes.

### 3.11 Desktop distribution

The standalone server remembers the signed-in user's Bunyip access token in memory and returns it at
`GET /desktop/registry-token` to that user's own session so the desktop updater can authenticate to the image
registry. A session without a token receives 503.

### 3.12 Background tasks

Started in `main`: idle-status scan (every 60 s; idle after 10 minutes), custom-status expiry (60 s), mute expiry
(60 s), email digest sender (hourly), orphan upload sweeper (hourly), unfurl image sweeper, scheduled-message dispatcher
(30 s), reminder dispatcher (30 s), poll closer (30 s), outgoing webhook delivery (10 s), room digest dispatcher
(30 min), embedding backfill (60 s, batches of 50), help-docs refresh (6 h), analytics aggregator (hourly, with a
start-up backfill), weekly recap (6 h tick, only when `LETS_CHAT_WEEKLY_RECAP` is enabled), message retention sweeper,
ephemeral-message sweeper (60 s), and the IMAP email poller.

## 4. Interfaces

The server exposes several hundred routes; this section lists their groups and the externally contractual ones. Route
definitions are in `server/src/routes/mod.rs` and the per-area route modules.

### 4.1 Operational

| Method | Path | Notes |
|--------|------|-------|
| GET | `/healthz` | Liveness, `{"status":"ok"}`; exempt from maintenance mode |
| GET | `/readyz` | Pings the three databases; 200 `ready` or 503 `unready`, with `auth_db`, `chat_db`, `settings_db`, `sso` booleans |
| GET | `/version` | `version`, `git_version`, `git_hash`, `build_date` |
| GET | `/ws` | WebSocket upgrade (session required) |

### 4.2 Browser application (session cookie)

Groups: home, rooms (`/room/{room_id}` and sub-routes for messages, threads, pins, wiki, manage, moderators, webhooks,
feeds, email inboxes, posting/broadcast/slowmode/retention policies, automations, stage and huddle tokens), messages
(`/messages/{message_id}` and sub-routes for edit, history, pin, bookmark, translate, forward, report, reactions,
acknowledgements), DMs (`/dm/{peer_id}`), enclaves (`/enclaves`, `/enclave/{id}/...`, `/invitations`), settings
(`/settings` and sub-routes for profile, appearance, language, theme, keywords, DND, emojis, canned responses, API
tokens, sessions, export, account deletion, blocked users), search (`/search`, `/searches`), reminders, scheduled
messages, transcripts (`/transcripts`, `/call/...`), uploads (`/api/upload`, `/api/files/{id}`), push (`/push/...`,
`/sw.js`), sign-in (`/login`, `/logout`, `/auth/bunyip/start|callback|approve`), and static assets.

### 4.3 JSON API v1 (bearer token)

`Authorization: Bearer <token>`; tokens are personal, expire optionally, can be revoked, and only their HMAC is stored.
A missing, expired or revoked token returns 401; a token lacking the needed scope returns 403. Scopes narrow the owner's
own access and never widen it.

| Method | Path | Scope |
|--------|------|-------|
| GET | `/api/v1/me` | any valid token |
| GET | `/api/v1/rooms` | `rooms:read` |
| GET, POST | `/api/v1/rooms/{room_id}/messages` | `messages:read`, `messages:write` |
| POST | `/api/v1/bridges/{bridge_id}/messages` | `bridge:post` |
| POST | `/api/v1/bridges/{bridge_id}/heartbeat` | `bridge:heartbeat` |

### 4.4 Public token routes

`POST /webhook/{secret}` (incoming webhook), `GET /feed/rss/{token}` and `GET /feed/ical/{token}`.

### 4.5 Admin (standalone build; `/admin/...`, admin role required)

Settings (IMAP, Giphy, LLM, remote control, transcription, help docs, email digest default), feature flags, maintenance,
users (ban, mute, role, quota, unlink SSO, delete), invites, rooms, enclaves, anti-spam, link filter, quarantine, backup
and restore, branding, moderation log, remote-control and voice logs, reports, support, bots, bridges, outgoing
webhooks, slash commands, analytics, and upload maintenance (regenerate thumbnails, purge orphans).

### 4.6 WebSocket protocol (`/ws`)

Client frames are JSON objects with a `type` field: `subscribe`, `subscribe_topic` (topics `enclave:{id}`, `user:{id}`,
`admin`, authorized per kind), `page_context`, `activity`, `pong`, `typing`, `thread_typing`, `wiki_editing`,
`call_signal`, `remote_control_signal`, `voice_join`, `voice_leave`, `voice_signal`, `voice_mute`, `voice_screen`,
`voice_reaction`, `voice_diag`, `stage_join`, `stage_leave`, `stage_raise_hand`, `stage_lower_hand`, `stage_step_down`,
`stage_promote`, `stage_demote`. The server sends rendered HTML fragments for events such as new, edited and deleted
messages, reactions, typing, status and profile changes, mentions, reminders, pins, polls, enclave membership changes
and admin badge updates, and a periodic ping that the client answers with `pong`.

### 4.7 Desktop shell

The shell opens the configured server URL in a webview and exposes IPC commands (including `set_server_url`) and an
injected bridge script for notifications and remote control.

## 5. Data model

Migration sets: auth `0001`-`0044`, chat `0001`-`0103`, settings `0001`-`0008` under `server/migrations/`. Migrations
are embedded and run at start; their files are immutable once released (enforced by a repository check script).

Key entities (by migration topic):

- auth: `users` (role, profile, status, locale, theme, density, Bunyip subject link, handle), `sessions` (hashed token,
  expiry, metadata), API tokens (hashed, scopes, expiry, revocation), push, APNs and FCM subscriptions, notification
  keywords, blocks, pending OIDC rows, login history and approvals, weekly recap state.
- chat: enclaves and `enclave_members` (role `owner`, `admin` or `member`), rooms (`room_type` default `public`,
  per-enclave unique names), messages (with FTS index, edits, threads, quotes, expiry, bridge actor), reactions,
  mentions, pins, bookmarks, uploads, link previews, polls and events, scheduled messages, reminders, drafts,
  custom emoji, user groups, room role overrides, incoming and outgoing webhooks, room feeds, email inboxes and reply
  tokens, bridges, thread followers and muters, saved searches, message reports, call transcripts and segments,
  acknowledgements, follow-ups, kudos, support tickets, remote-control sessions and events, voice events, message
  embeddings and doc chunks, analytics daily aggregates, storage quotas, branding.
- settings: key/value installation settings, VAPID keypair (private key sealed with the secret key), IMAP inbox
  configuration and poll status, anti-spam settings.

## 6. Configuration

Environment variables read by the server. Booleans accept `1`, `true`, `yes` (some also `on`). Optional services are
inactive when their variables are unset.

| Variable | Purpose |
|----------|---------|
| `BIND_ADDR` | Listen address (default `0.0.0.0:8080`; an unparseable value aborts) |
| `LETS_CHAT_DATA_DIR` | Directory holding the SQLite files (default `/data`); the `--data-dir <path>` command-line argument takes precedence |
| `LETS_CHAT_BASE_URL` | Public base URL used in links and emails (default `http://localhost:8080`) |
| `LETS_CHAT_ENVIRONMENT`, `PROD` | Environment label |
| `LETS_CHAT_BUNYIP_SSO_ISSUER`, `_CLIENT_ID`, `_CLIENT_SECRET`, `_REDIRECT_URI` | Required SSO settings (standalone) |
| `LETS_CHAT_BUNYIP_SSO_INSECURE_TLS` | Relaxes TLS verification for the SSO provider (development) |
| `LETS_CHAT_DEV_NO_SSO` | Disables SSO sign-in; nobody can sign in |
| `LETS_CHAT_SECRET_KEY` | Key for sealing two-factor secrets and stored API keys; unset disables two-factor authentication |
| `SETUP_DEFAULT_ADMIN` | Development bootstrap of a default admin |
| `LOGIN_APPROVAL_ENABLED` | Enable the suspicious-login approval gate |
| `IP2LOCATION_DB_PATH` | IP2Location database for login-location alerts |
| `LETS_CHAT_SMTP_HOST`, `_PORT`, `_TLS` (`tls`, `starttls`, `none`), `_FROM`, `_USERNAME`, `_PASSWORD` | Outbound mail relay (host, port, TLS mode and from address required together) |
| `LETS_CHAT_PUSH_CONTACT` | VAPID contact (default `mailto:admin@localhost`) |
| `LETS_CHAT_ICE_SERVERS` | JSON ICE server list for WebRTC (default one public STUN server) |
| `LETS_CHAT_LIVEKIT_URL`, `_API_KEY`, `_API_SECRET` | LiveKit SFU |
| `LETS_CHAT_TRANSCRIBE_AGENT_NAME`, `_TOKEN` | Transcription agent dispatch name and shared bearer |
| `LETS_CHAT_STT_PROVIDER`, `_URL`, `_API_KEY`, `_MODEL`, `_LANGUAGE`, `_PROMPT`, `_TIMEOUT_SECS`, `_WORKERS`, `_VAD_FILTER`, `_MAX_NO_SPEECH`, `_MIN_LOGPROB`, `_RATE_GLOBAL`, `_RATE_ROOM`, `_SCOPE` | Speech-to-text |
| `LETS_CHAT_LLM_URL`, `_API_KEY`, `_MODEL` | Summaries and other LLM features |
| `LETS_CHAT_EMBEDDINGS_URL`, `_API_KEY`, `_MODEL` | Embeddings |
| `LETS_CHAT_VISION_URL`, `_API_KEY`, `_MODEL` | Image description |
| `LETS_CHAT_AI_MODERATION` | AI moderation triage |
| `LETS_CHAT_GIPHY_API_KEY`, `LETS_CHAT_GIPHY_RATING` | Giphy search |
| `LETS_CHAT_WEEKLY_RECAP` | Enable weekly recap emails |
| `LETS_CHAT_RETENTION_SWEEP_ENABLED` | Enable the retention sweeper |
| `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED` | Enable the bridge avatar proxy |
| `RUST_LOG` | Log filter (default `lets_chat=info`) |

The command line accepts `--version` or `-V`, which prints the build banner and exits. IMAP inbox settings, Giphy key,
maintenance mode and similar settings are stored in the `settings` database and edited under `/admin`.

## 7. Security

- Browser sessions: opaque token in the `session` cookie, hashed at rest; the Bunyip identity-swap check; sessions can be
  revoked individually.
- Response headers on every response: HSTS (one year, includeSubDomains, preload), `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, a strict-origin referrer policy, a Content-Security-Policy restricting sources to the same
  origin (plus Giphy images and `ws:`/`wss:` connections), and a permissions policy allowing camera, microphone and
  display capture for the same origin only.
- Rate limits (per minute or cooldown, in memory): message, register, login, password reset, webhook, email inbox,
  email mention notification, remote-control request, bridge message, assistant ask, help-docs ask, support escalate,
  slowmode, new-member post, speech-to-text (global and per room), call reaction, voice diagnostics, voice mute, link
  unfurl and unfurl image.
- Outbound fetches (link unfurl, outgoing webhooks, slash commands) pass an SSRF guard that rejects targets resolving to
  non-globally-routable addresses, resolves through a public-only resolver to close DNS rebinding, rejects literal-IP
  URLs and re-checks every redirect hop.
- API tokens: HMAC-hashed at rest, scoped; bridge posting needs a dedicated scope and bridge accounts are restricted from
  ordinary read endpoints.
- Secrets sealed in the database (VAPID private key, Giphy key, two-factor secrets) use AES-256-GCM with a key derived
  from `LETS_CHAT_SECRET_KEY`.
- Input handling: MIME sniffing of uploads, message length cap, math rendering guarded by a control-sequence blocklist,
  bounded MIME parsing for inbound email, per-enclave and per-user storage quotas, anti-spam and link filtering.
- Admin and moderation actions are role checked and recorded in a moderation log.

## 8. Integrations and dependencies

- Bunyip: OpenID Connect identity provider (required for sign-in in the standalone build).
- LiveKit: SFU for huddles and stages; the transcription agent joins rooms as `agent-<name>`.
- Speech-to-text, LLM, embeddings and vision services reached over HTTP with API keys.
- SMTP relay (outbound mail) and IMAP mailbox (inbound email ingress, read with TLS using the Mozilla root list).
- Giphy for GIF search; web push services (VAPID), APNs and FCM.
- IP2Location LITE database file for login-location alerts.
- Transcription agent (`services/transcription-agent`): configured by `LIVEKIT_URL`, `LIVEKIT_API_KEY`,
  `LIVEKIT_API_SECRET`, `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN`, `LETS_CHAT_TRANSCRIBE_AGENT_NAME` (default `transcriber`)
  and `CLIP_MS` (default 5000); it posts roughly 5 second WAV clips to `/call/transcript/{id}/agent-clip` and runs no
  speech-to-text itself.
- Main crates: axum, sqlx, askama, fluent-templates, reqwest, tokio-rustls, async-imap, mail-parser, lettre,
  jsonwebtoken, totp-rs, aes-gcm, web-push-native, pulldown-cmark, pulldown-latex, syntect, image, zip.

## 9. Deployment and operations

- Image: `ci-build/Dockerfile.web`, built in `standalone` (default) or `saas` mode. The runtime stage is
  `debian:trixie-slim` with the binary, `server/assets` and `server/migrations`, running as user `appuser` (uid 1000).
  The working directory is `/app` because assets are served from the relative path `server/assets`.
- Port 8080 exposed; `LETS_CHAT_DATA_DIR=/data` volume; image health check polls `GET /healthz` every 30 seconds.
- `compose.yml` builds the image, publishes `127.0.0.1:8080`, mounts a named volume at `/data`, and expects an env file
  for SSO and optional features. Additional compose files provide development stacks (local, mock SSO, desktop).
- The transcription agent has its own `Dockerfile`; desktop builds use `ci-build/Dockerfile.desktop-*` for Linux and
  Windows bundles and installers.
- The SaaS binary mounts the `saas_auth` router in place of the Bunyip sign-in routes, the admin router and the desktop
  registry-token route.
- Upgrade notes are kept in `CHANGELOG.md`.

## 10. Non-functional behavior

- Logging: `tracing` with `EnvFilter`; build info (version, git hash, build date) is logged at start; background task
  failures log at warn or error level and the loops continue.
- Fail-fast startup: invalid pending restore, invalid secret key, invalid SSO configuration or SSO initialization failure
  exit with an error log; an unparseable bind address aborts.
- Optional subsystems degrade to a logged "disabled" state when unconfigured (mailer, push when no secret key, STT, LLM,
  embeddings, GeoIP).
- SQLite databases are opened with their own pools; readiness reports each database separately.
- Maintenance mode returns a maintenance page to non-admins while keeping assets, sign-in, SSO, version and health probes
  reachable.
- Live updates are in-process: a restart drops WebSocket connections, sessions persist in the auth database, and
  presence is recomputed from new connections.
