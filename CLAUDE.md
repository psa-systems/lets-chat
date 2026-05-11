# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**lets-chat** is a self-hosted fullstack chat application written entirely in Rust. The frontend is server-rendered HTML using Askama templates with HTMX for interactivity; the backend is Axum; persistence is split across three SQLite databases. It compiles to a single binary serving HTTP, WebSocket, and static assets.

## Build Modes

The server supports two mutually exclusive Cargo features:

### Standalone Mode (default)
Self-hosted deployment with built-in user management:
- Public `/register` page
- `/login` form-based password auth (Argon2id) with optional TOTP 2FA
- `/admin` pages (first registered user is auto-promoted to admin)

```bash
cargo build --release --features standalone -p lets-chat-server
# Binary: target/release/lets-chat
```

### SaaS Mode
Lightweight deployment integrated with a parent SaaS application:
- No `/register` route; users are provisioned by the parent app
- No `/admin` pages
- `/webhooks/maintenance` endpoint accepts maintenance signals from the parent app
- Identity is derived from the parent app's signed cookie (HMAC + JWT)

```bash
cargo build --release --no-default-features --features saas -p lets-chat-server
# Binary: target/release/lets-chat-saas
```

The two binaries share `src/main.rs`. Mode-specific code is gated with `#[cfg(feature = "standalone")]` or `#[cfg(feature = "saas")]`. The Docker image accepts `--build-arg BUILD_MODE=saas` to switch modes; the runtime stage always installs the binary as `/usr/local/bin/lets-chat` regardless of mode. `.env.standalone` and `.env.saas` document the required environment variables for each mode and must stay in sync when shared variables change.

## Commands

All common tasks are defined in `justfile`. Run `just --list` to see all recipes.

The host has no Rust or Bun installed. The recipes invoke the `./dev/cargo`, `./dev/cargo-desktop`, `./dev/bun`, and `./dev/server-up` wrappers, which run their tools inside Docker containers with persistent named volumes for the cargo registry and target directory.

```nu
# Development
just dev-web-local          # Local dev server (standalone) on http://localhost:18080
just dev-web-local-saas     # Local dev server (saas) on http://localhost:18080
just dev-web                # Docker dev (standalone) with Traefik at https://{USER}-chat.a8n.run
just dev-web-saas           # Docker dev (saas) with Traefik
just dev-web-down           # Stop Docker dev environment
just dev-desktop            # Desktop wrapper (Tao+Wry) pointed at the local server

# Checks & Formatting
just check                  # Run all checks: server (both modes), desktop, clippy (both modes), fmt
just fmt                    # cargo fmt --all

# Build
just build                  # Release binary, standalone (includes Tailwind CSS rebuild)
just build-saas             # Release binary, saas
just build-css              # Rebuild Tailwind CSS only (via bun)

# Tests
just test                   # cargo test --workspace (standalone)
just test-saas              # cargo test --workspace (saas)
just verify                 # Build release binary and verify GET /login returns 200 with a form

# Docker
just build-docker           # Build local Docker image (standalone)
just build-docker-saas      # Build local Docker image (saas)
just check-docker           # Validate standalone Docker image builds correctly
just check-docker-saas      # Validate saas Docker image builds correctly
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

## Inline script teardown

Inline `<script>` blocks in templates that attach listeners to `document`, `document.body`, or `window`, or that hold timers / `MutationObserver` instances, must register a teardown via `htmx:beforeCleanupElement` so that re-renders do not accumulate handlers. Element-bound listeners (on a textarea, button, or any node inside the fragment) do not need teardown; they go with the element on swap.

Canonical template:

```javascript
(function() {
  // setup
  function onSwap(evt) { /* body listener body */ }
  document.body.addEventListener('htmx:afterSwap', onSwap);

  // teardown
  function teardown() {
    document.body.removeEventListener('htmx:afterSwap', onSwap);
    // clearTimeout(...), observer.disconnect(), etc.
  }
  var root = document.currentScript.closest('[data-lc-cleanup-root]')
          || document.currentScript.parentElement;
  root.addEventListener('htmx:beforeCleanupElement', teardown, { once: true });
})();
```

Default root is `document.currentScript.parentElement`. When the script is a sibling of its logical owner (e.g. a form or wrapper div), tag the owner with `data-lc-cleanup-root` so the lookup picks it up. Examples in the codebase: `room/notify_dropdown.html`, `room/composer.html`.

### Why this matters in this codebase

Today there is **no `hx-boost` anywhere in this app** (sidebar links are plain `<a href>`, so room/DM navigation is a full browser reload that throws away all JS state). The only in-place re-render paths that reach inline scripts are:

1. The WS reconnect soft-refresh in `layout.html` (`htmx.ajax(..., {target:'#main', select:'#main'})`), which re-mounts everything under `#main`.
2. Targeted fragment swaps like the notify-dropdown's `hx-target="#lc-room-header" hx-swap="outerHTML"`.

So the accumulation is real but currently small. **If anyone ever adds `hx-boost`** (on the body or on sidebar links to make navigation feel snappier), the teardown pattern immediately becomes load-bearing: every sidebar click would re-mount `#main` and add one listener per inline script per navigation, and within minutes of use the app would be firing duplicate handlers for every event. Keep the teardown pattern intact; do not skip it on the grounds that "this script only runs once per page" - that invariant could disappear in a single PR.

### Persistent-shell exception

Scripts in `layout.html` are outside `#main` and run only at full browser page load - never re-rendered by any in-place swap in this app today. They intentionally have no teardown. Mark them with a one-line comment so future readers do not "fix" them by mistake:

```javascript
// Persistent-shell script: lives in layout.html, runs once per full browser
// page load, never re-rendered by in-place swaps. No htmx:beforeCleanupElement
// teardown needed; the listeners/state are intentionally lifetime-of-page.
```

If `hx-boost` is added later, re-evaluate these scripts too - they may need teardown then.
