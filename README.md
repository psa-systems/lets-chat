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

### Spaces and organization

- Enclaves: grouped rooms/workspaces, each with a default room and settings gear
- Room and DM mute, sidebar categories, starred rooms, and user groups
- Custom user status

### Notifications

- Web push notifications
- Email digest of missed mentions and DMs (off by default per user)

### Integrations and API

- JSON HTTP API v1 with scoped bearer tokens (see [`docs/api.md`](docs/api.md))
- First-class bot identities
- Incoming webhooks (post via secret URL) and outgoing webhooks (signed event subscriptions)

### Administration

- Admin panel: user management, room management, settings
- Analytics dashboard: DAU/MAU, messages, rooms, signups, retention
- Branding: custom logo, colors, login text, and favicon
- Anti-spam: rate limits, link filter, and honeypot
- Backup and restore archive
- Moderator tools: mute, ban, kick, delete messages
- Two-factor authentication (TOTP)
- Installable PWA with an offline message outbox
- Role-based access: Admin > Moderator > User

## Quick Start

### Docker (recommended)

```nu
docker build --tag lets-chat --file ci-build/Dockerfile.web .
docker run --publish 8080:8080 --volume lets-chat-data:/data lets-chat
```

Then open `http://localhost:8080`. The first registered account is automatically promoted to Admin.

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

Run `just --list` to see all available recipes.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | `/data` | Directory for SQLite `.db` files |
| `BIND_ADDR` | `0.0.0.0:8080` | Server listen address |
| `RUST_LOG` | `lets_chat=info` | Tracing filter |
| `LETS_CHAT_SECRET_KEY` | (none) | Encrypts at-rest secrets (Web Push VAPID key, 2FA TOTP secrets). See [`LETS_CHAT_SECRET_KEY`](#lets_chat_secret_key) below. |
| `LETS_CHAT_BASE_URL` | `http://localhost:8080` | Externally-reachable base URL. Used in outbound emails (password reset, email verification, digest deep links). |
| `LETS_CHAT_ICE_SERVERS` | `[{"urls":"stun:stun.l.google.com:19302"}]` | JSON array of `RTCIceServer` objects for WebRTC calls and voice channels. Add a TURN entry for reliable NAT traversal. |
| `LETS_CHAT_PUSH_CONTACT` | `mailto:admin@localhost` | VAPID contact address sent with Web Push delivery requests. |
| `LETS_CHAT_SERVER_URL` | `http://localhost:8080` | URL the desktop wrapper opens. Server-only deployments can ignore it. |
| `LETS_CHAT_UPDATE_URL` | `https://dev.a8n.run/api/packages/a8n-tools/generic/lets-chat` | Forgejo Generic Packages root the desktop self-updater reads. |
| `SMTP_HOST` / `SMTP_PORT` / `SMTP_TLS` / `SMTP_FROM` / `SMTP_USERNAME` / `SMTP_PASSWORD` | (none) | SMTP relay configuration. All five non-credential vars must be set together to enable outbound mail. Username+password are an optional pair. |

### `LETS_CHAT_SECRET_KEY`

Encrypts at-rest secrets used by features that store sensitive data: Web Push (VAPID private key) and 2FA (per-user TOTP secrets). Future encrypted-at-rest features will reuse the same key. (SMTP credentials are passed via environment variables, not the database, so they do not depend on this key.)

**Format.** Any non-empty string. The server SHA-256-hashes it to derive a 32-byte AES-256-GCM key, so length and encoding don't matter; entropy does. Use at least 32 random bytes.

**Generate one:**

```sh
head -c 32 /dev/urandom | base64
```

(or `openssl rand -base64 32` if OpenSSL is handy.)

**Without it.** Push and 2FA are silently disabled. Settings shows the relevant checkboxes as disabled with help text pointing back here. The rest of the app works normally.

**If you lose it.** Encrypted rows become undecryptable, but the rest of the app continues to run.

- *Web Push:* existing browser subscriptions become orphaned (the server can no longer sign messages for them). Users re-subscribe automatically on their next @-mention or DM after a fresh keypair is generated.
- *2FA:* enrolled users can't log in. Recovery requires clearing `totp_secret_encrypted`, `totp_nonce`, `totp_enabled`, and `totp_recovery_hashes` for affected users in `auth.db`; they then re-enroll.

**If you rotate it.** The app does NOT auto-regenerate encrypted rows. On startup with a new key:

- *Web Push:* the VAPID keypair fails to decrypt and a `vapid keypair load failed` warning is logged. Push stays disabled until the row is cleared and a fresh keypair generated:
  ```sh
  sqlite3 /data/settings.db "DELETE FROM vapid_keypair;"
  ```
  After restart, browser subscriptions issued under the old keypair are invalid; users may need to clear site data or unregister the service worker before a new subscription takes hold.
- *2FA:* same lockout as the lost-key case above; clear the affected `users` columns to unblock login.

**Storage.** Treat it like a database password. Use Docker `--env-file`, your deployment's secret manager, or a `.env` file with restricted permissions. Don't bake it into a committed `compose.yml`.

## Email digests

Sends each opted-in user one email summarising mentions and DMs they missed while offline.

### Operator setup

Three things must be configured for the feature to be fully functional.

1. **SMTP environment variables**. The same set used by password reset and email verification. All required to enable outbound mail:
   - `SMTP_HOST`
   - `SMTP_PORT`
   - `SMTP_TLS` (one of `tls` / `starttls` / `none`)
   - `SMTP_FROM`
   - `SMTP_USERNAME` and `SMTP_PASSWORD` together (optional pair; both unset means the relay is opened unauthenticated)
2. **`LETS_CHAT_BASE_URL`** in the environment, e.g. `https://chat.example.com`. Used to construct clickable deep links in the email body. Defaults to `http://localhost:8080` if unset; the digest still sends with that URL but the links will only work for local development.
3. **(Optional) "New users start with email digest enabled"** at `/admin/settings`. Off by default. Flipping it on only affects users who register after the flip; existing users are unchanged. Users can override their own preference at `/settings`.

Changes to SMTP env vars take effect on the next server restart.

### User opt-in

1. Sign in and go to `/settings`.
2. Enter and verify an email address (the existing email-verification flow).
3. Tick "Email me a digest of missed mentions and DMs".
4. Save preferences.

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
- **Styles**: Tailwind CSS (compiled via Bun)
- **Desktop**: Optional Tao + Wry native webview wrapper in `desktop/`
