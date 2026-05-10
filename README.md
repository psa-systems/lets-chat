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

### `LETS_CHAT_SECRET_KEY`

Encrypts at-rest secrets used by features that store sensitive data: today, Web Push (VAPID private key) and 2FA (per-user TOTP secrets). Future encrypted-at-rest features will reuse the same key.

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

## Tech Stack

- **Frontend**: Server-rendered HTML via [Askama](https://github.com/djc/askama) templates + [HTMX](https://htmx.org/) for interactivity
- **Backend**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **Database**: SQLite via SQLx (three separate pools: auth, chat, settings)
- **Real-time**: WebSocket hub broadcasting pre-rendered HTML fragments via `hx-swap-oob`
- **Styles**: Tailwind CSS (compiled via Bun)
- **Desktop**: Optional Tao + Wry native webview wrapper in `desktop/`
