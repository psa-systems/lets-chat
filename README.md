# Let's Chat

A self-hosted fullstack chat application built in Rust. Server-rendered HTML via Askama + HTMX over an Axum backend, compiled to a single binary serving HTTP, WebSocket, and static assets.

## Features

- Public chat rooms with real-time messaging
- Direct messages between users
- Message editing with live updates
- Typing indicators
- Emoji reactions and read receipts
- Full-text message search
- Moderator tools: mute, ban, kick, delete messages
- Admin panel: user management, room management, SMTP settings
- Email digest of missed mentions and DMs (off by default per user)
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
| `LETS_CHAT_SECRET_KEY` | (none) | Encrypts at-rest secrets (Web Push VAPID key, 2FA TOTP secrets, SMTP password). See [`LETS_CHAT_SECRET_KEY`](#lets_chat_secret_key) below. |

### `LETS_CHAT_SECRET_KEY`

Encrypts at-rest secrets used by features that store sensitive data: Web Push (VAPID private key), 2FA (per-user TOTP secrets), and SMTP (admin-configured outbound password). Future encrypted-at-rest features will reuse the same key.

**Format.** Any non-empty string. The server SHA-256-hashes it to derive a 32-byte AES-256-GCM key, so length and encoding don't matter; entropy does. Use at least 32 random bytes.

**Generate one:**

```sh
head -c 32 /dev/urandom | base64
```

(or `openssl rand -base64 32` if OpenSSL is handy.)

**Without it.** Push, 2FA, and email digests are silently disabled. Settings shows the relevant checkboxes as disabled with help text pointing back here. The rest of the app works normally.

**If you lose it.** Encrypted rows become undecryptable, but the rest of the app continues to run.

- *Web Push:* existing browser subscriptions become orphaned (the server can no longer sign messages for them). Users re-subscribe automatically on their next @-mention or DM after a fresh keypair is generated.
- *2FA:* enrolled users can't log in. Recovery requires clearing `totp_secret_encrypted`, `totp_nonce`, `totp_enabled`, and `totp_recovery_hashes` for affected users in `auth.db`; they then re-enroll.
- *SMTP:* the stored password cannot be decrypted; the admin SMTP page renders a banner and digest sending stops. Re-enter the password at `/admin/settings` and restart.

**If you rotate it.** The app does NOT auto-regenerate encrypted rows. On startup with a new key:

- *Web Push:* the VAPID keypair fails to decrypt and a `vapid keypair load failed` warning is logged. Push stays disabled until the row is cleared and a fresh keypair generated:
  ```sh
  sqlite3 /data/settings.db "DELETE FROM vapid_keypair;"
  ```
  After restart, browser subscriptions issued under the old keypair are invalid; users may need to clear site data or unregister the service worker before a new subscription takes hold.
- *2FA:* same lockout as the lost-key case above; clear the affected `users` columns to unblock login.
- *SMTP:* the password fails to decrypt and a `smtp settings load failed` warning is logged. Re-enter the password at `/admin/settings` and restart.

**Storage.** Treat it like a database password. Use Docker `--env-file`, your deployment's secret manager, or a `.env` file with restricted permissions. Don't bake it into a committed `compose.yml`.

## Email digests

Sends each opted-in user one email summarising mentions and DMs they missed while offline.

### Operator setup

Four things must be configured for the feature to be fully functional. Each is independent except where noted.

1. **`LETS_CHAT_SECRET_KEY`** in the environment. Required to encrypt the SMTP password. Without it the admin form refuses to save, and the digest tick is a no-op.
2. **SMTP at `/admin/settings`**. Fill in host, port, username, password, from address, and TLS mode (STARTTLS / TLS / None). Save.
3. **Public site URL at `/admin/settings`**. Externally-reachable base URL (e.g. `https://chat.example.com`). Used to construct clickable deep links in the email body. If left empty the digest still sends but items are not clickable.
4. **(Optional) "New users start with email digest enabled"** at `/admin/settings`. Off by default. Flipping it on only affects users who register after the flip; existing users are unchanged. Users can override their own preference at `/settings`.

After saving SMTP changes, **restart the server** for the new settings to take effect. The dispatch path and the "Send test email" button both read a startup snapshot of SMTP config.

To verify SMTP is working:

1. Save settings, restart.
2. At `/admin/settings`, enter a recipient in the "Send test email" form and click **Send test**.
3. A banner reports success or the underlying SMTP error verbatim.

### User opt-in

1. Sign in and go to `/settings`.
2. Enter an email address in the "Email address for digests" field.
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

### Upgrade note

The migration that introduced encrypted SMTP storage **discarded any previously stored plaintext password**. After upgrading from a version prior to phase 22:

1. Restart with `LETS_CHAT_SECRET_KEY` set.
2. Open `/admin/settings`.
3. Re-enter the SMTP password and save.
4. Restart again so the dispatch path picks up the new config.

Other SMTP fields (host, port, username, from address) survived the upgrade and do not need to be re-entered.

## Tech Stack

- **Frontend**: Server-rendered HTML via [Askama](https://github.com/djc/askama) templates + [HTMX](https://htmx.org/) for interactivity
- **Backend**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **Database**: SQLite via SQLx (three separate pools: auth, chat, settings)
- **Real-time**: WebSocket hub broadcasting pre-rendered HTML fragments via `hx-swap-oob`
- **Styles**: Tailwind CSS (compiled via Bun)
- **Desktop**: Optional Tao + Wry native webview wrapper in `desktop/`
