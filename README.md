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
| `LETS_CHAT_SECRET_KEY` | (none) | AES-256-GCM key for encrypting SMTP password at rest |

## Tech Stack

- **Frontend**: Server-rendered HTML via [Askama](https://github.com/djc/askama) templates + [HTMX](https://htmx.org/) for interactivity
- **Backend**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **Database**: SQLite via SQLx (three separate pools: auth, chat, settings)
- **Real-time**: WebSocket hub broadcasting pre-rendered HTML fragments via `hx-swap-oob`
- **Styles**: Tailwind CSS (compiled via Bun)
- **Desktop**: Optional Tao + Wry native webview wrapper in `desktop/`
