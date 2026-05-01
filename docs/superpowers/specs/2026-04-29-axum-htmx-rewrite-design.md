# Axum + HTMX Rewrite Design

**Date:** 2026-04-29
**Status:** Approved (brainstorming)
**Branch (planned):** `feat/axum-htmx-rewrite`

## Problem

Page load and refresh take several minutes in production because the browser must download, parse, and instantiate a multi-megabyte Dioxus WASM bundle before the UI becomes interactive. Investigation across multiple sessions concluded the cost is intrinsic to Dioxus's fullstack/hydration model and not addressable through bundle splitting, lazy loading, or code-size tuning while keeping Dioxus as the frontend.

## Goal

Replace the Dioxus WASM frontend with a server-rendered HTML application using Axum + Askama + HTMX. Initial page payload drops from megabytes of WASM to tens of kilobytes of HTML. No WASM compile step, no hydration. Backend Axum stays. SQLite databases, migrations, session model, role model, WebSocket hub, and `ChatEvent` broadcast pattern are preserved.

## Non-Goals

- Visual redesign. UI should look and behave the same.
- Schema or migration changes.
- Auth model changes (cookies, sessions, RBAC tiers).
- Multi-tenancy, federation, or new feature work.
- Native desktop UI written in Rust. Desktop becomes a webview wrapper.

## Architecture

Classic server-rendered MPA with HTMX progressive enhancement.

- **Initial request:** Axum handler authenticates via cookie, queries SQLite, renders an Askama template, returns full HTML. Page contains the layout, sidebar, current view, vendored HTMX scripts, and Tailwind CSS link.
- **Form submissions and in-page actions:** HTMX issues `hx-post`, `hx-patch`, or `hx-delete` requests. Handlers return HTML fragments which HTMX swaps into the DOM. Targets identified by stable DOM ids.
- **Live updates:** the page opens a single WebSocket via `htmx-ext-ws`. Server broadcasts pre-rendered HTML fragments using `hx-swap-oob` to update messages, reactions, typing indicators, and unread counts in real time. Client sends typing pings; all other client-to-server actions go over normal HTTP.

### Tech Stack

- **Axum 0.8** — HTTP server, routing, WebSocket upgrade. Already in use.
- **Askama 0.12** — compile-time-checked templates. `.html` files in `templates/`, struct definitions in `src/views/`.
- **HTMX 2.x** plus extensions `ws`, `response-targets`, `morph` (idiomorph). Vendored under `assets/vendor/`.
- **Tailwind CSS** — kept. Built by Bun, output `assets/tailwind-built.css`.
- **Tao + Wry** — desktop webview wrapper (separate workspace member).
- **SQLx, axum-extra (cookies), argon2, tracing, dashmap, time, tower, tower-http** — kept; tower/tower-http are added or made explicit.

### Removed

- `dioxus`, `wasm-bindgen`, `web-sys`, `js-sys`, `gloo-timers`, `dx` CLI, `Dioxus.toml`.
- WASM build target.
- All `#[server]` macros, all `#[component]` functions, the `client`/`desktop` Cargo features, and the `feature = "server"` cfg gates that exist solely to separate WASM from native code.

### Result

Initial HTML payload on the order of 10 to 50 KB. No WASM compile or load. Page render time bounded by network and SQLite query time, not WASM hydration.

## Module Layout

```
src/
├── main.rs              # Axum entry; builds router; starts server
├── lib.rs               # Re-exports for tests
├── state.rs             # AppState (3 SQLite pools + Hub)
├── auth.rs              # Cookie middleware; require_auth extractor
├── routes/              # One file per route group
│   ├── mod.rs
│   ├── auth.rs          # GET/POST /login, /register, /logout
│   ├── home.rs          # GET /
│   ├── room.rs          # GET /room/:id; POST /room/:id/messages; etc.
│   ├── dm.rs
│   ├── invite.rs
│   ├── admin.rs
│   ├── reactions.rs
│   ├── moderation.rs
│   └── search.rs
├── views/               # Askama template structs
│   ├── mod.rs
│   ├── layout.rs
│   ├── room.rs
│   ├── dm.rs
│   ├── auth.rs
│   ├── admin.rs
│   └── partials.rs
├── models/              # Kept: User, Message, Room, Reaction, etc.
├── db/                  # Kept: auth.rs, chat.rs, settings.rs
└── ws/                  # Kept; events emit rendered HTML

templates/               # Askama .html files mirror views/
├── layout.html
├── auth/
├── room/
├── dm/
├── admin/
└── partials/

assets/
├── tailwind-built.css   # Kept
├── main.css             # Kept
└── vendor/
    ├── htmx.min.js
    ├── htmx-ext-ws.js
    ├── htmx-ext-response-targets.js
    └── idiomorph.js

desktop/                 # New workspace member
├── Cargo.toml
└── src/main.rs          # Tao+Wry webview pointing at server URL
```

Root crate is renamed to `lets-chat-server`. Desktop crate is `lets-chat-desktop`.

Files removed: `src/components/`, `src/routes.rs`, `src/server_fns/` (replaced by `src/routes/`), `Dioxus.toml`.

## Page Rendering and Form Flow

### Full-page request

`GET /room/abc`:
1. Cookie middleware extracts session, loads `User`. Missing or expired session redirects to `/login` with 303.
2. Handler queries `db::chat` for the room, recent messages (paged), and member list.
3. Handler renders `RoomPage { user, room, messages, ... }` and returns `Html(rendered)` with status 200.
4. Page contains `<div hx-ext="ws" ws-connect="/ws">` near `<body>`, so HTMX opens the live socket on load.

### HTMX form submission

Send a message:
1. Composer is `<form hx-post="/room/abc/messages" hx-target="#composer" hx-swap="outerHTML">`.
2. Handler validates input, inserts into `messages` table, broadcasts `ChatEvent::MessageSent` to the hub.
3. Handler returns an empty composer fragment to clear the input. HTMX swaps `#composer`.
4. Hub fan-out delivers the rendered message HTML to all subscribers including the sender, who appends the message via OOB swap.

### HTMX in-place partial

Delete a message:
1. `<button hx-delete="/messages/123" hx-target="#msg-123" hx-swap="outerHTML">`.
2. Handler authorizes, deletes, broadcasts `ChatEvent::MessageDeleted`, returns "[deleted]" fragment.
3. WS broadcast updates the same message in other clients.

### Fragment vs full page

Each route handler inspects the `HX-Request` header. If present, it returns a fragment template. Otherwise it returns a full page. `partials/` templates are reused in both modes plus in WS broadcasts, so a single template defines the canonical HTML for any given UI element.

### Error handling

Validation errors return a fragment with `hx-swap-oob` for an error banner plus a 4xx status. The client uses `htmx-ext-response-targets` (`hx-target-4*="#err"`) to route errors to a separate target without affecting the success target.

## WebSocket Flow

### Connection lifecycle

1. Page loads with `<div hx-ext="ws" ws-connect="/ws">` near the body root.
2. HTMX opens the WS connection. The Axum upgrade handler authenticates from the session cookie. Unauthenticated upgrades close immediately.
3. The hub stores a `user_id -> WsSender` mapping plus a per-connection active context (room id or DM peer id).
4. After the page loads it sends an `ws-send` HTMX message: `{type: "subscribe", context: "room:abc"}`. The server updates the connection's active context.

### Server to client events

Each `ChatEvent` variant maps to a rendered HTML fragment with HTMX swap directives:

```html
<!-- new message: appended to message list -->
<div id="messages" hx-swap-oob="beforeend">
  <div id="msg-42">...</div>
</div>

<!-- message edited: replace by id -->
<div id="msg-42" hx-swap-oob="outerHTML">edited body</div>

<!-- typing indicator: replace inner -->
<div id="typing" hx-swap-oob="innerHTML">alice is typing</div>

<!-- reaction added: replace reaction bar -->
<div id="reactions-42" hx-swap-oob="outerHTML">...</div>
```

The hub renders the fragment once and broadcasts it to all relevant subscribers. Because the sender also receives the broadcast, the POST handler does not need to return the message body itself; it only returns a cleared composer fragment.

### Client to server messages

The only WS-originated message is the typing ping: `{type: "typing", room: "abc"}`. All other actions (send, edit, delete, react, kick, ban, etc.) are normal HTMX HTTP requests.

### Reconnect

`htmx-ext-ws` reconnects with exponential backoff. On reconnect, the client re-sends its subscribe message.

## Auth and Sessions

Unchanged in DB schema and cookie format. Session token is a random opaque string, stored in `auth.db`, served as HTTP-only Secure SameSite=Strict cookie with 30-day expiry. The first registered user is auto-promoted to Admin. Roles: Admin > Moderator > User.

A new `axum::middleware::from_fn` reads the cookie and injects `Option<User>` into request extensions. Pages requiring auth use `require_auth(State, Extension)` which returns `Result<User, Redirect>` and 303s to `/login` on miss.

Login and register forms use plain `<form method="post">`. They also work over HTMX: on success the handler returns `HX-Redirect: /` so HTMX performs a client-side navigation; on failure it returns the form fragment with errors. Non-HTMX clients get a 303 redirect on success.

## Desktop Wrapper

The `desktop/` workspace member compiles to `lets-chat-desktop`, a small native binary using Tao for the window and Wry for the webview.

- Reads `LETS_CHAT_SERVER_URL` from the environment (e.g. `https://chat.example.com`).
- Opens a single OS window pointing the webview at that URL.
- Roughly 50 lines. No login UI, no business logic. Cookie storage handled by the webview.
- Linux GTK environment variables (`GDK_BACKEND=x11`, `GDK_SCALE`, `GDK_DPI_SCALE`) move from `src/main.rs` into the desktop binary.
- Build: `cargo build -p lets-chat-desktop --release`.
- Dev: `just dev-desktop` runs the desktop binary against a locally running server.

## Build, Test, Tooling

### Build

- `cargo build --release` produces the server binary directly. No `dx` CLI. No WASM target.
- `bun run build` (existing) produces `assets/tailwind-built.css`.
- Static assets served via `tower-http::services::ServeDir` mounted at `/assets`.

### Tests

- Existing in-memory SQLite database tests are preserved unchanged.
- New: handler-level tests using `axum::Router::oneshot` plus assertions on the rendered HTML body (assert that fragments contain the expected ids and text).
- `just verify` updated to fetch `/login` and assert that the body contains `<form` rather than WASM hydration markers.

### Justfile

- `dev-web-local` becomes `cargo run` (or `cargo watch -x run` if `cargo-watch` is installed).
- `dev-web` keeps Docker compose semantics but no longer needs `dx serve`.
- `dev-desktop` becomes `LETS_CHAT_SERVER_URL=http://localhost:8080 cargo run -p lets-chat-desktop`.
- `build-css` unchanged.
- `check-client` is removed. `check-desktop` is added: `cargo check -p lets-chat-desktop`.
- `check` aggregates: server check, desktop check, clippy, fmt.

### Docker

- The web `Dockerfile` simplifies to: `cargo chef` prepare/cook layer, `bun build` for CSS, `cargo build --release`, copy binary plus assets to a slim runtime image. No multi-stage WASM build.
- `compose.yml` and `compose.dev.yml` are updated to drop any WASM-specific volumes, build args, or labels.

## Migration Sequence

Big-bang rewrite on `feat/axum-htmx-rewrite`. One commit per phase.

1. **Workspace and deps.** Convert root crate to a workspace; add `desktop/` member. Replace Dioxus deps with Askama, tower, tower-http, futures-util. Remove WASM-only deps. Add HTMX scripts and `idiomorph.js` to `assets/vendor/`. Collapse Cargo features to a single binary. Verify `cargo check` passes.
2. **App skeleton.** Rewrite `src/main.rs` to build the Axum router with placeholder handlers, serve `/assets/*`, open three SQLite pools, and construct `AppState`. Keep `db/`, `models/`, `ws/events.rs`. Verify the server binds and `GET /` returns 200.
3. **Layout and auth.** Add `templates/layout.html`, `templates/auth/{login,register}.html`. Implement `/login`, `/register`, `/logout`, cookie middleware, `require_auth` extractor. Verify: first registered user becomes admin, login sets the cookie, logout clears it.
4. **Sidebar and home.** Add the sidebar partial (rooms list, DMs list). Implement `GET /` (welcome page).
5. **Room view (read path).** `GET /room/:id` renders messages, member list, composer. No live updates yet, no send. Verify scrollback loads.
6. **WebSocket.** Wire `/ws` upgrade with cookie auth. Port the hub to broadcast pre-rendered HTML fragments. `htmx-ext-ws` connects on page load. Subscribe message updates the active context. Verify two browser tabs receive typing pings.
7. **Send, edit, delete messages.** `POST/PATCH/DELETE` endpoints. Hub broadcasts. Composer clears via OOB swap. Verify multi-tab message flow.
8. **Reactions.** Reaction bar partial; `POST /messages/:id/reactions/:emoji` toggle; `ReactionUpdate` HTML broadcast.
9. **DMs.** Mirror room implementation for DM threads.
10. **Search.** `GET /search?q=...` returns a results fragment. HTMX-driven search-as-you-type with `hx-trigger="input changed delay:200ms"`.
11. **Admin pages.** Users, invites, rooms, mod log, settings. Mostly tables and forms.
12. **Moderation actions.** Kick, ban, timeout, unban. Mod log entries broadcast via WS for live admin updates.
13. **Read receipts and auto-scroll.** Track last-read; OOB swaps update unread badges in the sidebar. Auto-scroll via small inline JS or `htmx-on::after-swap`.
14. **Desktop wrapper.** Tao+Wry binary in `desktop/`. Smoke-test against a running server.
15. **Docker and CI.** Rewrite `Dockerfile`. Update `compose.yml` and `compose.dev.yml`. `just check`, `just test`, `just verify` all green.
16. **Cleanup.** Delete `Dioxus.toml`, old `components/`, old `routes.rs`, WASM target config. Delete stale plans (`disable-ssr-hydration.md`, `chat-auto-scroll.md`, `server-client-build-split.md`). Update `CLAUDE.md`.
17. **Merge.** PR review and merge to `main`.

## Risks and Open Questions

- **Asset versioning / cache busting.** `tower-http`'s `ServeDir` does not fingerprint filenames. Will use a query-string version suffix (`?v=<git-short-sha>`) embedded in the layout template at compile time via an Askama variable populated from a build-script-derived constant.
- **Idiomorph vs default swap.** Default to standard HTMX swap. Use `idiomorph` only where DOM diffing matters (sidebar with selected room highlighting, message list when reactions update mid-list). Decide per-component during implementation.
- **WS auth on browser refresh.** The cookie travels on the upgrade request; existing pattern in `ws/handler.rs` already validates it. No change required, but confirm during phase 6.
- **Search performance.** `GET /search` is rendered server-side per keystroke. With debounce + LIMIT, expected acceptable for current scale. Revisit if SQLite FTS becomes a bottleneck.

## Out of Scope (Deferred)

- Visual redesign.
- Native push notifications on desktop.
- Mobile app shell.
- Multi-tenant or multi-server federation.
