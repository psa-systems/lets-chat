# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**lets-chat** is a self-hosted fullstack chat application written entirely in Rust. The frontend is server-rendered HTML using Askama templates with HTMX for interactivity; the backend is Axum; persistence is split across three SQLite databases. It compiles to a single binary serving HTTP, WebSocket, and static assets.

## Commands

All common tasks are defined in `justfile`. Run `just --list` to see all recipes.

The host has no Rust or Bun installed. The recipes invoke the `./dev/cargo`, `./dev/cargo-desktop`, `./dev/bun`, and `./dev/server-up` wrappers, which run their tools inside Docker containers with persistent named volumes for the cargo registry and target directory.

```nu
# Development
just dev-web-local          # Local dev server in a container at http://localhost:18080
just dev-web                # Docker dev with Traefik at https://{USER}-chat.a8n.run
just dev-web-down           # Stop Docker dev environment
just dev-desktop            # Desktop wrapper (Tao+Wry) pointed at the local server

# Checks & Formatting
just check                  # Run all checks: server, desktop, clippy, fmt
just fmt                    # cargo fmt --all

# Build
just build                  # Release binary (includes Tailwind CSS rebuild)
just build-css              # Rebuild Tailwind CSS only (via bun)

# Tests
just test                   # cargo test --workspace (uses in-memory SQLite pools)
just verify                 # Build release binary and verify GET /login returns 200 with a form

# Docker
just build-docker           # Build local Docker image
just check-docker           # Validate Docker image builds correctly
```

### Running a single test

```nu
./dev/cargo test -p lets-chat-server --test db_auth test_name
```

Test files live in `server/tests/` and use in-memory SQLite pools - no setup required.

## Architecture

### Technology Stack

- **Frontend**: Server-rendered HTML via Askama templates with HTMX for interactivity.
- **Backend**: Axum 0.8 + tower-http; HTTP and WebSocket served from the same process.
- **Databases**: Three SQLite files via SQLx with async pools - `auth.db`, `chat.db`, `settings.db`. Migrations in `server/migrations/{auth,chat,settings}/`.
- **Real-time**: WebSocket hub at `/ws`. The server broadcasts pre-rendered HTML fragments with `hx-swap-oob` so HTMX merges live updates without client-side rendering logic.
- **Desktop**: Optional Tao+Wry webview wrapper in `desktop/`.

### Code Layout

```
server/
|-- src/
|   |-- main.rs            # Axum entry: tracing, AppState, listener
|   |-- lib.rs             # pub re-exports for tests
|   |-- state.rs           # AppState (3 SQLite pools + Hub)
|   |-- auth.rs            # Cookie middleware + extractors
|   |-- error.rs           # AppError + IntoResponse
|   |-- routes/            # Per-area HTTP handlers
|   |-- views/             # Askama template structs
|   |-- models/            # Shared data types
|   |-- db/                # SQLx access per domain
|   `-- ws/                # Hub + ChatEvent enum
|-- templates/             # Askama .html files
|-- assets/                # main.css, tailwind input/output, vendored htmx
|-- migrations/            # Per-domain SQLite migrations
`-- tests/                 # Integration tests
desktop/                   # Tao + Wry wrapper
```

### Auth & Sessions

- Sessions are random tokens stored in `auth.db`, served as HTTP-only `Secure SameSite=Strict` cookies with 30-day expiry.
- The `AuthUser` and `AdminUser` Axum extractors in `server/src/auth.rs` read the cookie and resolve the `User` (or reject the request).
- First registered user is auto-promoted to Admin.
- Roles: Admin > Moderator > User. RBAC logic lives in `server/src/db/auth.rs`.

### WebSocket Flow

1. Client connects to `/ws` after login; the handler authenticates via the session cookie.
2. The browser uses the `htmx-ext-ws` extension. The server pushes pre-rendered HTML fragments tagged with `hx-swap-oob` attributes; HTMX merges them into the DOM without any client-side JSON-to-DOM translation.
3. Mutations (send message, edit, react, mark-read) go through normal HTTP handlers, which write to the database and then call `hub.broadcast(room_id, event)` to fan out the rendered fragment.
4. The hub maintains a `DashMap<RoomId, Vec<UnboundedSender<...>>>`; each connection subscribes/unsubscribes as the user navigates between rooms.

### Database Domain Separation

Each database has its own SQLx pool and is initialized independently at startup. The three pools are carried in `AppState` (`server/src/state.rs`) and shared with handlers via Axum's state extractor. Cross-domain lookups (e.g., fetching a `User` when rendering messages) require querying multiple pools.

## Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | `/data` | Directory for SQLite `.db` files |
| `BIND_ADDR` | `0.0.0.0:8080` | Server listen address |
| `RUST_LOG` | `lets_chat=info` | Tracing filter |
| `LETS_CHAT_SECRET_KEY` | (none) | AES-256-GCM key for encrypting TOTP secrets at rest (SHA-256 of this string is the 32-byte key). Unset or empty disables two-factor authentication entirely - the setup page and login challenge return 404, and the enforcement middleware is a no-op. |
| `LETS_CHAT_SERVER_URL` | `http://localhost:8080` | URL the desktop wrapper opens |

## Workspace Layout

`Cargo.toml` at the repo root defines a Cargo workspace with two members:

- `server/` - the Axum application (binary `lets-chat`, library `lets_chat`).
- `desktop/` - a thin Tao+Wry webview that loads `LETS_CHAT_SERVER_URL`.

There are no Cargo features for platform selection. Server-only code lives under `server/`, desktop-only code under `desktop/`.

## Tailwind CSS

Tailwind is compiled by Bun (`server/package.json` scripts). The output `server/assets/tailwind-built.css` is gitignored and regenerated by `just build-css` or `just build`. Run `just build-css` after changing class names if the dev server does not pick them up automatically.
