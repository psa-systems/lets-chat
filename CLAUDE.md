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
| `LETS_CHAT_SECRET_KEY` | (none) | AES-256-GCM key for encrypting SMTP password in settings |
| `LETS_CHAT_SERVER_URL` | `http://localhost:8080` | URL the desktop wrapper opens |
| `LETS_CHAT_ICE_SERVERS` | `[{"urls":"stun:stun.l.google.com:19302"}]` | JSON array of `RTCIceServer` objects for 1:1 WebRTC calls. Add a TURN entry for reliable NAT traversal. |
| `LETS_CHAT_UPDATE_URL` | `https://dev.a8n.run/api/packages/a8n-tools/generic/lets-chat` | Forgejo Generic Packages root the desktop self-updater reads. The updater fetches `${URL}/latest/latest.json` for the manifest and downloads platform binaries from `${URL}/${version}/lets-chat-desktop-{linux,windows}-x86_64[.exe]`. Override to test against a fork or a local fixture (eg. `http://127.0.0.1:18180`). |

## Workspace Layout

`Cargo.toml` at the repo root defines a Cargo workspace with two members:

- `server/` - the Axum application (binary `lets-chat`, library `lets_chat`).
- `desktop/` - a thin Tao+Wry webview that loads `LETS_CHAT_SERVER_URL`.

There are no Cargo features for platform selection. Server-only code lives under `server/`, desktop-only code under `desktop/`.

## Tailwind CSS

Tailwind is compiled by Bun (`server/package.json` scripts). The output `server/assets/tailwind-built.css` is gitignored and regenerated by `just build-css` or `just build`. Run `just build-css` after changing class names if the dev server does not pick them up automatically.

## Test maintenance

Test files under `server/tests/` each open their own in-memory SQLite pools and construct their own `AppState`. There is no shared `tests/common/` helper. This means any change to `AppState`'s shape, to the migration set, to a `#[cfg]`-gated route module, or to a handler's request/response contract can silently rot tests that touch the affected surface. Tests are integration binaries: a test file that fails to compile prevents none of the OTHER test files from running, so a single quietly-broken file can sit unnoticed across multiple phases until someone audits.

**Test files compiling AND passing is a precondition for landing a phase PR, not optional.** Run `just test` and `just test-saas` before opening the PR. If either fails or refuses to compile a binary, that is in scope for the phase that introduced the change, not "later cleanup."

Phase 24 was the cleanup pass that surfaced four drift categories actively rotting `server/tests/`:

### 1. AppState construction drift

**Trigger.** Adding a field to `AppState` in `server/src/state.rs`. Production startup at `server/src/main.rs` is updated; tests that construct `AppState { ... }` by hand still use the old field set and fail to compile with `error[E0063]: missing field <name>`.

**Prevention.** When adding a field to `AppState`:
```nu
grep --recursive --line-number 'AppState {' server/tests/
```
Update every match. The canonical construction shape is at `server/src/main.rs` (search for `let state = AppState {`); most existing test files already use a near-mirror with mock clients in place of real ones. Many tests need to construct any background-task handles (e.g., `let bg = lets_chat::bg::spawn(auth.clone());`) before the struct literal, so add that line at the same time.

**Detection.** `just check` (or `./dev/cargo check -p lets-chat-server`) surfaces every site mechanically. Don't merge if any site is red.

### 2. Migration-list drift

**Trigger.** Adding a `.sql` file under `server/migrations/{auth,chat,settings}/`. Production picks it up automatically via the migrator at startup; each test file's hand-rolled `setup_*_pool()` helper hardcodes the migration set as `include_str!(...)` calls and does not.

**Two patterns coexist.** Don't pretend only one shape exists:

- **Array form (most files).** A `for sql in [include_str!(...), include_str!(...), ...]` array, executed in a single loop. Adding a new migration means appending one more `include_str!(...)` line inside the array. Indentation varies by file: some use 8-space (function-level), some 12-space (nested inside a struct or match arm).
- **Verbose per-migration form (`db_dm.rs`, `db_moderation.rs`, `message_editing.rs`).** Each migration is its own `let chat_mN = include_str!(...);` followed by `sqlx::raw_sql(chat_mN).execute(&pool).await.expect("chat migration N");`. Adding a new migration means appending an analogous 5-line block before the function return.

The two patterns are functionally equivalent and intentionally not consolidated; whichever a file currently uses is what new migrations get added to.

**Prevention.** When adding a migration:
```nu
grep --recursive --line-number '<previous_migration_filename>' server/tests/
```
For each match, eyeball the surrounding code to determine the pattern, then add the new migration in the matching shape. Don't grep-replace blindly — array form takes 1 line, verbose form takes 5.

**Detection.** `just test` (and `just test-saas`). A missed migration usually surfaces as `SqliteError { code: 1, message: "table X has no column named Y" }` at runtime; downstream HTTP tests see this as a 500 in route handlers.

### 3. Feature-gate drift

**Trigger.** Wrapping a route module, a route registration, or a code branch in `#[cfg(feature = "standalone")]` (or `#[cfg(feature = "saas")]`) in production without mirroring the same `#[cfg]` on the test file that exercises that surface. Standalone-only tests then build and run in saas mode, hit a missing route (404) or a different code path (different status), and fail. Same in reverse for saas-only tests.

**Prevention.** Whenever you add or change a `#[cfg(feature = "...")]` in `server/src/routes/` or `server/src/main.rs`:
- If the gate is on a whole module (`#[cfg(feature = "standalone")] mod admin;` in `routes/mod.rs`), any test file that exercises routes from that module gets the same gate at file scope: `#![cfg(feature = "standalone")]` as the first non-blank line.
- If the gate is on a single code branch inside a handler (e.g., the password-check block in `routes/account.rs::post_delete_account`), gate only the tests that assert on that branch, with a per-test `#[cfg(feature = "standalone")]` attribute above `#[tokio::test]`. Don't whole-file-gate when only some tests in the file are affected — the other tests still need to run in both modes.

**Detection.** Only surfaces when both `just test` and `just test-saas` are run. Standalone passing is not sufficient. A 404-vs-303 assertion failure for an admin/saas-only route is the typical signature.

### 4. Test-contract drift

**Trigger.** Rewriting a handler's request shape (form field names, query parameters) or response shape (status code, body content type, redirect vs. HTML fragment) without updating the tests that POST to it. Common during HTMX-isation: a handler that used to return `Redirect::to(...)` now returns `Result<Html, AppError>` so the typeahead UI can swap a row in place, and any test that asserted `res.status().is_redirection()` quietly breaks.

**Prevention.** Unlike the first three categories, this one cannot be caught mechanically — `just check` is clean because the test compiles fine; only running the test surfaces the failure. The discipline is:
- When you change a handler's status code, redirect target, form field names, or response body shape, search for tests that hit the route:
  ```nu
  grep --recursive --line-number '/your/route/path' server/tests/
  ```
- Update each test's request body and response assertion at the same time as the handler change.
- A short inline comment in the test explaining *why* the contract is what it is (HTMX fragment vs. redirect, for example) helps the next reader avoid re-litigating.

**Detection.** Only by running the affected tests. Add the test update to the same commit as the handler change; don't defer.

### Other notes

- The `server/tests/` directory contains 50 integration binaries. A flake in one (current: `routes_uploads`'s upload-pipeline tests under concurrent-binary load) is not a drift category and is not in scope for "make tests pass."
- Two `setup_*_pool()` patterns coexist in test files (array vs. verbose). Consolidating them is a clean follow-up hygiene task; not in scope for any given feature phase.
