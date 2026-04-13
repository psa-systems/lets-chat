# Let's Chat

A self-hosted fullstack chat application built with Dioxus (Rust + WASM) and Axum. Compiles to a single binary serving both the API and the WASM client.

## Features

- Public chat rooms with real-time messaging
- Direct messages between users
- Message editing with live updates
- Typing indicators
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

**Prerequisites:** Rust toolchain with `wasm32-unknown-unknown` target, [Dioxus CLI](https://dioxuslabs.com/) v0.7.x, [Bun](https://bun.sh/)

```nu
just dev-web-local
```

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
| `LETS_CHAT_SECRET_KEY` | — | AES-256-GCM key for encrypting SMTP password at rest |

## Tech Stack

- **Frontend**: [Dioxus](https://dioxuslabs.com/) 0.7 — Rust compiled to WASM
- **Backend**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **Database**: SQLite via SQLx (three separate pools: auth, chat, settings)
- **Real-time**: WebSocket hub with room subscriptions and event fan-out
- **Styles**: Tailwind CSS (compiled via Bun)
