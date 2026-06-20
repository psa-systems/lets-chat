# Let's Chat

A self-hosted fullstack chat application built in Rust. Server-rendered HTML via Askama + HTMX over an Axum backend, compiled to a single binary serving HTTP, WebSocket, and static assets.

![Let's Chat][1]

[1]: lets-chat.png

## Features

### Messaging

- Public chat rooms and private/invite-only rooms with real-time messaging
- Direct messages between users
- Message editing with live updates and an edit-history drawer
- Typing indicators, read receipts, and message grouping
- Emoji reactions, including custom emoji
- Full-text message search
- Pinned messages and per-user bookmarks
- Polls and voting
- Scheduled message delivery: pick a future time in the composer, see and edit pending sends at `/scheduled`
- Message reminders ("remind me about this message")
- Slash commands
- File and image uploads with link unfurling
- `@`-mentions, broadcast mentions, and a notifications inbox

### Voice and video

- 1:1 audio and video calls over WebRTC, with mic/camera/speaker selection
- Multi-party enclave voice channels
- Consent-gated remote control: during a 1:1 call, request keyboard/mouse control of a peer's screen (desktop app; verified-email gated, revocable at any time)

### Spaces and organization

- Enclaves: grouped rooms/workspaces, each with a default room and settings gear
- Room and DM mute, sidebar categories, starred rooms, and user groups
- Custom user status

### Personalization

- Light, dark, and high-contrast (WCAG AA/AAA) themes, plus a "follow system" option; saved per user at `/settings`
- Comfortable or compact display density
- Localized UI with an in-app language picker (English and Spanish), falling back to the browser language (see [`docs/i18n.md`](docs/i18n.md))

### Notifications

- Web push notifications
- Email digest of missed mentions and DMs (off by default per user)
- Per-mention and per-DM notification emails (off by default per user); reply to one of those emails to post that reply to chat as yourself

### Integrations and API

- JSON HTTP API v1 with scoped bearer tokens (see [`docs/api.md`](docs/api.md))
- First-class bot identities
- Incoming webhooks (post via secret URL) and outgoing webhooks (signed event subscriptions)
- Email ingress: per-room IMAP-poll inboxes that turn an `<token>@<ingress-domain>` address into chat posts (see [`docs/email-ingress.md`](docs/email-ingress.md))
- Per-room Atom and iCal feeds, each served from a revocable secret-token URL

### Administration

- Admin panel: user management, room management, settings
- Analytics dashboard: DAU/MAU, messages, rooms, signups, retention
- Branding: custom logo, colors, login text, and favicon
- Anti-spam: rate limits, link filter, and honeypot
- Backup and restore archive
- Moderator tools: mute, ban, kick, delete messages
- Single sign-on: "Sign in with Bunyip" (OIDC) is the sole authentication path (LC-22); there is no local username/password, registration, password reset, or 2FA
- Installable PWA with an offline message outbox
- Role-based access: Admin > Moderator > User

## Quick Start

### Docker (recommended)

```nu
docker build --tag lets-chat --file ci-build/Dockerfile.web .
docker run --publish 8080:8080 --volume lets-chat-data:/data \
  --env LETS_CHAT_BUNYIP_SSO_ISSUER=https://your-op.example.com \
  --env LETS_CHAT_BUNYIP_SSO_CLIENT_ID=... \
  --env LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET=... \
  --env LETS_CHAT_BUNYIP_SSO_REDIRECT_URI=https://chat.example.com/auth/bunyip/callback \
  lets-chat
```

Then open `http://localhost:8080` and click "Sign in with Bunyip". The first user to sign in is automatically promoted to Admin.

> Authentication is Bunyip SSO only (LC-22): the four `LETS_CHAT_BUNYIP_SSO_*` vars are **mandatory** and the server refuses to start without them. The OIDC client must be registered on your Bunyip OP with the `redirect_uri` above in its `redirect_uris`. There is no local-auth fallback.

> **Upgrading?** Read [`CHANGELOG.md`](CHANGELOG.md) first. It calls out default-on behavior changes, security fixes, and env vars you may need to set before upgrading. The convention behind it is in [`docs/releasing.md`](docs/releasing.md).

### Local Development

The host needs only Docker, [just](https://github.com/casey/just), and (optionally) [Nushell](https://www.nushell.sh/) for the `verify` recipe. Cargo and Bun run inside containers via the wrappers in `dev/`.

```nu
just dev-web-local
```

Then open `http://localhost:18080`.

Or with Docker + Traefik (requires a configured domain):

```nu
just dev-web
```

`just dev-web` serves at `https://${USER}-chat.a8n.run` behind the shared Traefik and needs a working Bunyip OP. The compose file (`compose.dev-web.yml`) wires the SSO vars and two dev-only escape hatches; first-time setup also requires, outside this repo:

1. **An OIDC client for lets-chat on the dev Bunyip OP.** Register a confidential `client_secret_basic` client whose `redirect_uris` includes `https://${USER}-chat.a8n.run/auth/bunyip/callback`, then put its id/secret in `compose.dev-web.yml`. The committed bunyip-api seed only registers the static staging host, so the per-developer dev redirect must be added against the running dev OP DB.
2. **A hosts entry so the browser resolves the app over the Nebula overlay.** Add `${USER}-chat.a8n.run` to the same `/etc/hosts` line as the other dev hosts (the Nebula IP), or the name falls through to public DNS and Traefik returns 404.

Two dev-only flags in `compose.dev-web.yml` cover the dev OP's posture and are documented inline there: `LETS_CHAT_BUNYIP_SSO_INSECURE_TLS=1` (the dev OP serves a self-signed cert; reqwest's bundled rustls roots reject it with no env-only remedy) and an `extra_hosts` pin of the issuer host to the dev Traefik container (server-to-server SSO calls must hit the Nebula-side Traefik, not the public IP). Neither is for production.

Run `just --list` to see all available recipes.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | `/data` | Directory for SQLite `.db` files |
| `BIND_ADDR` | `0.0.0.0:8080` | Server listen address |
| `RUST_LOG` | `lets_chat=info` | Tracing filter |
| `LETS_CHAT_BUNYIP_SSO_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` / `_REDIRECT_URI` | (none) | Bunyip OIDC SSO, the sole sign-in path (LC-22). All four are **mandatory**; the server refuses to start without them. `_REDIRECT_URI` is `{base}/auth/bunyip/callback` and must match a `redirect_uris` row on the OP client. Discovery + JWKS are fetched at startup and must succeed. |
| `LETS_CHAT_BUNYIP_SSO_INSECURE_TLS` | (unset = strict) | DEV ONLY. `1`/`true` makes the SSO HTTP client accept invalid TLS certs (for a dev OP behind a self-signed cert). Never set in production: it disables issuer TLS authentication. |
| `LETS_CHAT_SECRET_KEY` | (none) | Encrypts at-rest secrets (Web Push VAPID private key; the sealed IMAP password for email ingress). See [`LETS_CHAT_SECRET_KEY`](#lets_chat_secret_key) below. |
| `LETS_CHAT_BASE_URL` | `http://localhost:8080` | Externally-reachable base URL. Used to build deep links in outbound mail (digest, mention/DM notifications) and the SSO redirect. |
| `LETS_CHAT_ICE_SERVERS` | `[{"urls":"stun:stun.l.google.com:19302"}]` | JSON array of `RTCIceServer` objects for WebRTC calls and voice channels. Add a TURN entry for reliable NAT traversal. |
| `LETS_CHAT_STT_URL` / `_API_KEY` / `_MODEL` | (none) | Optional **server-side** call transcription. Point `_URL` at an OpenAI-compatible `/v1/audio/transcriptions` endpoint (whisper.cpp server, faster-whisper, LocalAI, ...) to transcribe calls server-side instead of with the in-browser Web Speech engine: browser-agnostic (Firefox/Safari) and keeps audio off third-party clouds. `_API_KEY` is an optional bearer token; `_MODEL` defaults to `whisper-1`. Unset = the in-browser engine. The endpoint is operator-trusted and **not** SSRF-filtered, so it may be `localhost`/internal; never point it at an untrusted host. |
| `LETS_CHAT_LLM_URL` / `_API_KEY` / `_MODEL` | (none) | Optional AI **summaries** for saved call transcripts. Point `_URL` at an OpenAI-compatible `/v1/chat/completions` endpoint (Ollama, llama.cpp server, vLLM, LocalAI, ...) to show a "Summarize" action (summary + action items, cached) on the transcript page. `_API_KEY` is an optional bearer token; `_MODEL` defaults to `gpt-4o-mini`. Unset = the action is hidden. Operator-trusted and **not** SSRF-filtered (may be `localhost`/internal); never point it at an untrusted host. |
| `LETS_CHAT_PUSH_CONTACT` | `mailto:admin@localhost` | VAPID contact address sent with Web Push delivery requests. |
| `LETS_CHAT_SERVER_URL` | `http://localhost:8080` | URL the desktop wrapper opens. Server-only deployments can ignore it. |
| `LETS_CHAT_UPDATE_URL` | `https://dev.a8n.run/api/packages/a8n-tools/generic/lets-chat` | Forgejo Generic Packages root the desktop self-updater reads. Every fetch and each redirect hop is validated against a public-IP SSRF filter. |
| `LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE` | (unset = off) | Exempts only the initial `LETS_CHAT_UPDATE_URL` from the SSRF filter (private internal mirror or loopback test fixture). Redirect targets are still validated. |
| `LETS_CHAT_SMTP_HOST` / `LETS_CHAT_SMTP_PORT` / `LETS_CHAT_SMTP_TLS` / `LETS_CHAT_SMTP_FROM` / `LETS_CHAT_SMTP_USERNAME` / `LETS_CHAT_SMTP_PASSWORD` | (none) | SMTP relay configuration. All five non-credential vars must be set together to enable outbound mail. Username+password are an optional pair. |
| `LETS_CHAT_RETENTION_SWEEP_ENABLED` | (unset = disabled) | Set to `1` or `true` to enable the destructive hard-delete sweep that enforces per-room message `retention_days`. Read once at startup; flipping it requires a restart. |
| `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED` | (unset = enabled) | LC-78-AVATAR-PROXY. When enabled (the default), a bridge daemon may submit a `foreign_avatar` URL: the server fetches it once, magic-byte-sniffs and re-encodes it through the uploads pipeline, and serves it same-origin. Set to `false`/`0`/`no`/`off` to restore v1's posture (reject any non-null `foreign_avatar` with HTTP 400). Read per request. |

### `LETS_CHAT_SECRET_KEY`

Encrypts at-rest secrets used by features that store sensitive data: Web Push (VAPID private key) and the sealed IMAP password for email ingress. Future encrypted-at-rest features will reuse the same key. (SMTP credentials and the Bunyip SSO client secret are passed via environment variables, not the database, so they do not depend on this key.)

**Format.** Any non-empty string. The server SHA-256-hashes it to derive a 32-byte AES-256-GCM key, so length and encoding don't matter; entropy does. Use at least 32 random bytes.

**Generate one:**

```sh
head -c 32 /dev/urandom | base64
```

(or `openssl rand -base64 32` if OpenSSL is handy.)

**Without it.** Web Push and encrypted email ingress are silently disabled. Settings shows the relevant checkboxes as disabled with help text pointing back here. The rest of the app works normally.

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

## Email digests

Sends each opted-in user one email summarising mentions and DMs they missed while offline.

### Operator setup

Three things must be configured for the feature to be fully functional.

1. **SMTP environment variables**. The same set used by all outbound mail (digest and mention/DM notifications). All required to enable outbound mail:
   - `LETS_CHAT_SMTP_HOST`
   - `LETS_CHAT_SMTP_PORT`
   - `LETS_CHAT_SMTP_TLS` (one of `tls` / `starttls` / `none`)
   - `LETS_CHAT_SMTP_FROM`
   - `LETS_CHAT_SMTP_USERNAME` and `LETS_CHAT_SMTP_PASSWORD` together (optional pair; both unset means the relay is opened unauthenticated)
2. **`LETS_CHAT_BASE_URL`** in the environment, e.g. `https://chat.example.com`. Used to construct clickable deep links in the email body. Defaults to `http://localhost:8080` if unset; the digest still sends with that URL but the links will only work for local development.
3. **(Optional) "New users start with email digest enabled"** at `/admin/settings`. Off by default. Flipping it on only affects users who first sign in after the flip; existing users are unchanged. Users can override their own preference at `/settings`.

Changes to SMTP env vars take effect on the next server restart.

### User opt-in

1. Sign in and go to `/settings`. Your email address comes from your Bunyip account (the SSO `email` claim).
2. Tick "Email me a digest of missed mentions and DMs".
3. Save preferences.

Users with no email address on file are skipped by the digest tick regardless of the checkbox state. Muted rooms (`mute_mode = 'all'`) and muted DMs are excluded; `mute_mode = 'except_mentions'` rooms still contribute their mentions.

### Delivery semantics

- **Cadence**: hourly background tick. First fire one hour after server start.
- **Quiet period**: 1 hour. The user must have had neither HTTP activity (`last_active_at`) nor WebSocket activity (`last_ws_seen_at`) within the last hour.
- **One digest per offline session**: the tick gates on `last_digest_sent_at < MAX(last_active_at, last_ws_seen_at)`. As soon as the user comes back online and bumps either column, the gate self-resets, so the next offline session is eligible for one more email.
- **Time window**: 7 days. Activity older than 7 days never appears in a digest.
- **Item cap**: 50 across all sections combined. Overflow renders as a "... and N more" footer.
- **Subject**: `[lets-chat] N new mentions and M direct messages` (zero clauses dropped).
- **Format**: `multipart/alternative` with both `text/plain` and `text/html` parts.

## Tech Stack

- **Frontend**: Server-rendered HTML via [Askama](https://github.com/djc/askama) templates + [HTMX](https://htmx.org/) for interactivity
- **Backend**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **Database**: SQLite via SQLx (three separate pools: auth, chat, settings)
- **Real-time**: WebSocket hub broadcasting pre-rendered HTML fragments via `hx-swap-oob`
- **Styles**: Tailwind CSS (compiled via Bun), with semantic design tokens driving light/dark/high-contrast themes
- **i18n**: [Project Fluent](https://projectfluent.org/) catalogs embedded at compile time (see [`docs/i18n.md`](docs/i18n.md))
- **Desktop**: Optional Tao + Wry native webview wrapper in `desktop/`
