# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**lets-chat** is a self-hosted fullstack chat application written entirely in Rust. The frontend is server-rendered HTML using Askama templates with HTMX for interactivity; the backend is Axum; persistence is split across three SQLite databases. It compiles to a single binary serving HTTP, WebSocket, and static assets.

## Commands

All common tasks are defined in `justfile`. Run `just --list` to see all recipes.

The host has no Rust or Bun installed. One-shot commands (`cargo`, `cargo` for the desktop crate, `bun`) run through the `./dev/cargo`, `./dev/cargo-desktop`, and `./dev/bun` wrapper scripts; long-running dev services (`dev-web*`, `dev-desktop`) run through Docker Compose files named `compose.dev-<recipe>.yml` (so `just dev-web-local` reads `compose.dev-web-local.yml`, and so on). Both paths use persistent named volumes for the cargo registry, git checkout cache, build target dir, and `/data` so rebuilds stay incremental and SQLite state survives across restarts.

```nu
# Development (each starts a `docker compose --file compose.dev-<recipe>.yml` stack)
just dev-web-local          # Local dev server at http://localhost:18080 (cargo run from source)
just dev-web                # Production-shape build behind Traefik at https://{USER}-chat.a8n.run
just dev-web-local-down     # Stop the local stack
just dev-web-down           # Stop the Traefik-fronted stack
just dev-desktop            # Desktop wrapper (Tao+Wry) pointed at http://localhost:18080

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
3. Mutations (send message, edit, react, mark-read) go through normal HTTP handlers, which write to the database and then broadcast a `ChatEvent` (`server/src/ws/events.rs`) to fan out the rendered fragment.
4. The hub (`server/src/ws/hub.rs`) holds `connections: DashMap<ConnId, Connection>` plus three fan-out indexes: `rooms: DashMap<i64, HashSet<ConnId>>` (room subscriptions), `topics: DashMap<String, HashSet<ConnId>>` (typed topics, LC-160), and `user_conns: DashMap<String, HashSet<ConnId>>` (a user's tabs). Fan-out methods: `broadcast_to_room`, `broadcast_to_topic`, `broadcast_to_user`, `broadcast_global`.
5. The per-connection send task in `server/src/routes/ws.rs` receives each `ChatEvent` and renders it to an OOB fragment **for that recipient** (so per-viewer state - `can_edit`, `can_manage`, unread counts - is correct), or to `None` to skip. Recipient-independent events render via `views::ws_fragments::render_event`.

### Live updates by default (LC-156 epic)

A new page should be live "by construction." The conventions that make that work:

- **Declarative subscription.** A page element carries `data-lc-live-room="<id>"` or `data-lc-live-topic="<topic>"`; `server/assets/live.js` sends the subscribe frame on socket open / htmx settle. Topics are `enclave:{id}`, `user:{id}`, `admin`; `ws.rs::topic_subscribe_allowed` authorizes each kind at subscribe time (enclave membership or site-admin; own id; admin role).
- **Two fan-out shapes.** Per-user surfaces (own profile, saved list, invitations) use `broadcast_to_user` - no topic needed, the WS arm gates on `user_id == send_user.id`. Shared surfaces (enclave member/room lists, admin user/room lists) use `broadcast_to_topic`.
- **id-keyed OOB regions are self-limiting.** The live fragment swaps an element by id (e.g. `#lc-enclave-settings-members`, `#sidebar-self`, `#sidebar-nav-{enclave_id}`, `#lc-saved-list`, `#user-{id}`). A connection whose current page lacks that id silently drops the swap - so correctness does **not** depend on knowing each connection's current page, and stale subscriptions are harmless. Enclave-scoped regions are keyed by enclave id so a viewer of a different enclave never gets the wrong list.
- **Shared partial.** The list/region body is an Askama partial included by BOTH the full page and the OOB fragment (`enclave/members_items.html`, `partials/sidebar_self.html`, `saved/items.html`, ...), so the live update and a fresh page load render identically. Per-page row-builders are factored into `pub(crate)` helpers shared by the page handler and the WS render.
- **Admin surfaces are `#[cfg(standalone)]`.** `routes::admin` is standalone-only, so the WS arms + renderers for `AdminUserChanged`/`AdminRoomChanged` are gated; in saas the events never fire and fall through to `render_event` (None). Mirror this for any future admin-only live surface or `just test-saas` breaks.
- **Topic cleanup on access loss (LC-176).** `Hub::unsubscribe_user_from_topic` is called from kick/leave/enclave-delete so a departed member stops receiving an enclave's events immediately (not just on disconnect).
- **Paginated / filtered pages use a refresh affordance, not a full swap (LC-179).** `/inbox` (infinite-scroll) and `/activity` (tab-filtered) carry view-state the server can't see, so a full-list swap would clobber it. They reveal a hidden "refresh" bar over the WS instead; clicking reloads the current URL.

### Database Domain Separation

Each database has its own SQLx pool and is initialized independently at startup. The three pools are carried in `AppState` (`server/src/state.rs`) and shared with handlers via Axum's state extractor. Cross-domain lookups (e.g., fetching a `User` when rendering messages) require querying multiple pools.

### Migration files are immutable once shipped (LC-212)

Once a `.sql` file under `server/migrations/{auth,chat,settings}/` lands on `main`, never edit it. Comments count. The `sqlx::migrate!` macro records a BLAKE3 checksum of each file's full content into `_sqlx_migrations` on first apply, and on every subsequent start it recomputes that checksum and compares. Any difference - including a comment swap - panics startup with `migration N was previously applied but has been modified` and the container restart-loops until the file is reverted or the operator hand-edits `_sqlx_migrations`. LC-212 was exactly this: a one-line comment update inside a shipped migration broke deploy on every operator whose database had already recorded the old hash.

If a migration needs new behaviour, add a new `.sql` file with the next index. If documentation drifts after the fact (e.g., an env-var rename like LC-201 invalidating prose in an older migration's comments), update the doc comment in the code that reads the env (`Mailer::from_env`, `Config::from_env`, etc.) or add a forward-pointing note in a new migration's comment block. Do not retouch the historic file. This is a one-way ratchet and it applies to every migrator that hashes files (refinery, golang-migrate, alembic, flyway), not only sqlx; treat the rule as universal.

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
| `LETS_CHAT_RETENTION_SWEEP_ENABLED` | (unset = disabled) | Gates `spawn_message_retention_sweeper`. Set to `1` or `true` to enable the destructive hard-delete sweep that enforces per-room `retention_days`. Off by default while the strict-vs-loose semantics question for thread retention is open with the ticket author; current behavior is loosest-correct (sweep-by-newest-reply preserves active threads). Flipping requires a server restart; the spawn function reads the var at startup and does not poll. |

## Email ingress (LC-77)

`spawn_email_poll` polls an operator-configured IMAP mailbox every 5 minutes; messages addressed to `<token>@<ingress-domain>` post to their room as the `MessageActor::EmailInbox` synthetic actor. See `docs/email-ingress.md` for the operator deployment guide, threat model, and failure-log taxonomy. Gated at startup on `LETS_CHAT_SECRET_KEY` set + `imap_inbox_config.enabled = 1` + `imap_inbox_config.ingress_domain` set; flipping `enabled` requires a server restart (same precedent as the retention sweeper). The IMAP password is AES-256-GCM-sealed at rest under `LETS_CHAT_SECRET_KEY` via `db::imap_config`; per-room inbox secrets are HMAC-SHA256-hashed via `db::email_inbox`. The named link-filter skip decision (mirroring the LC-74 webhook posture) is anchored in a code comment in `routes::room::finalize_email_inbox_message_send`.

### Per-message notification emails and reply-by-email (LC-77-REPLY, #201)

Stage 1 (notification emails): `crate::email::notification::dispatch_mention_notification` fires fire-and-forget from the 5 mention-reconcile / DM-send hook sites in `routes::room`. Per-user opt-in (`users.notify_email_activity_enabled`, default 0), per-recipient rate cap (20/min via `RateLimitKind::EmailMentionNotification`), 7-day reply-token TTL in `chat.db::reply_tokens`. Outbound mail carries `Reply-To: reply-<token>@<ingress-domain>` (when ingress is configured) plus `Auto-Submitted: auto-generated` to break reciprocal loops.

Stage 2 (reply ingress): the IMAP poll loop's namespace-forked resolver (`email_ingress::resolve::resolve_address`) routes `reply-<token>@<ingress-domain>` addresses to `email_ingress::reply_actor::post_reply_message`. The actor mirrors the HTTP `post_message` posting gates (banned/muted, room access, posting policy, DM block, message rate cap), strips the quoted-original and signature via `strip_quoted_reply`, posts as the real user, and consumes the token on success (one-shot replay defense). Gate failures leave the token in place so the user can recover after the gating condition lifts. See `docs/email-ingress.md` for the threat model and the namespace-fork rules.

### Exactly-once dedup (LC-77-MID-DEDUP, #202)

`db::email_ingress_dedup` records the HMAC-SHA256 (under `LETS_CHAT_SECRET_KEY`) of every successfully-posted message's RFC 5322 `Message-ID:` header in `chat.db::processed_message_ids`. `process_polled_message` checks the table BEFORE resolving so a wire-byte-identical replay (the crash-between-process-and-STORE-Seen race) drops with `DropReason::Duplicate` instead of posting again. The table is opaque (hashes only; no plaintext leak). Sweep at 30 days piggybacks on the hourly orphan sweeper. A message without a `Message-ID:` header falls back to v1 at-least-once.

### Dead-letter folder (LC-77-DEAD-LETTER, #203)

Optional `imap_inbox_config.dead_letter_folder` column. When set, `poll_once` issues `UID COPY <uid> <folder>` for every dropped UID (including FETCH-fail and oversize-payload pre-process drops) before marking `\Seen` on the source. COPY failures are logged INFO under `target=email_ingress::dead_letter` and never block the `\Seen` STORE so a misconfigured folder cannot stop the queue. The operator must pre-create the folder at the IMAP provider; lets-chat does not auto-create it. Empty field = feature off (v1 always-`\Seen` posture; structured drop log is the only diagnostic).

## Workspace Layout

`Cargo.toml` at the repo root defines a Cargo workspace with two members:

- `server/` - the Axum application (binary `lets-chat`, library `lets_chat`).
- `desktop/` - a thin Tao+Wry webview that loads `LETS_CHAT_SERVER_URL`.

There are no Cargo features for platform selection. Server-only code lives under `server/`, desktop-only code under `desktop/`.

## Tailwind CSS

Tailwind is compiled by Bun (`server/package.json` scripts). The output `server/assets/tailwind-built.css` is gitignored and regenerated by `just build-css` or `just build`. Run `just build-css` after changing class names if the dev server does not pick them up automatically.

## Test maintenance

Test files under `server/tests/` each open their own in-memory SQLite pools and construct their own `AppState`. Most files (40+) use shared pool helpers at `server/tests/common/mod.rs` (`common::chat_pool()` and siblings, backed by `sqlx::migrate!` so new migrations land automatically); a smaller set hand-rolls the migration list and is drift-prone (see category 2). This means any change to `AppState`'s shape, to the migration set, to a `#[cfg]`-gated route module, or to a handler's request/response contract can silently rot tests that touch the affected surface. Tests are integration binaries: a test file that fails to compile prevents none of the OTHER test files from running, so a single quietly-broken file can sit unnoticed across multiple phases until someone audits.

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

**Three patterns coexist.** Don't pretend only one shape exists:

- **Common helpers via `sqlx::migrate!` (drift-immune).** 40+ files do `mod common;` and call `common::chat_pool().await` (or `common::auth_pool()` / `common::settings_pool()`) from `server/tests/common/mod.rs`. The macro picks up every `.sql` file in the migrations dir at compile time, so new migrations land in these tests automatically. A small number of files (e.g. `db_dm.rs`) inline `sqlx::migrate!` directly without going through `common` and are equivalently immune. **Prefer this pattern for new test files** so the migration-list drift category does not apply at all (the cure for migration-list drift is to stop maintaining the list).
- **Array form (drift-prone).** 17 files hand-list migrations via a `for sql in [include_str!(...), include_str!(...), ...]` array, executed in a single loop. Adding a new migration means appending one more `include_str!(...)` line inside the array. Indentation varies: some use 8-space (function-level), some 12-space (nested inside a `match` arm).
- **Verbose per-migration form (drift-prone).** 1 file: `db_private_rooms.rs`. Each migration is its own 5-line `sqlx::raw_sql(include_str!(...)).execute(&pool).await.expect("migration N")` block. The per-block label numbering is off-by-one against the migration filename; continue that convention rather than re-numbering everything.

The three patterns are functionally equivalent and intentionally not consolidated; whichever a file currently uses is what new migrations get added to.

**Prevention.** When adding a migration:
```nu
grep --recursive --line-number '<previous_migration_filename>' server/tests/
```
For each match, eyeball the surrounding code to determine the pattern, then add the new migration in the matching shape. Don't grep-replace blindly: array form is one line, verbose form is one 5-line block, common-helper files do not match the grep at all and need no edit. New test files should use `common::*_pool()` and skip this whole category.

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

### 5. Test-harness setup gotchas

Test binaries that exercise authed endpoints need two setup patterns or they fail in non-obvious ways:

1. **`enforce_2fa_enrollment` middleware activates when `AppState.secret_key.is_some()`.** Users with `totp_enabled = 0` get 303'd to `/settings/2fa/setup` on every authed request. Test harness workaround: `UPDATE users SET totp_enabled = 1` on every created user after registration.
2. **`db::enclave::backfill_general_membership` early-returns when no admin exists.** Tests that downgrade users to `'user'` role leave them out of the General enclave; all room access then 403s. Test harness must promote at least one user to `'admin'` before backfill runs.

Both surfaces are invisible until they fail. Reference: `routes_mentions.rs` (admin promotion pattern), `routes_message_edit_history.rs` (totp_enabled pattern). The symptoms - 303 to `/settings/2fa/setup` and 403 on `POST /room/1/messages` respectively - are both the kind of error that looks like a routing or auth bug but is actually a setup omission.

### Other notes

- The `server/tests/` directory contains 50 integration binaries. A flake in one (current: `routes_uploads`'s upload-pipeline tests under concurrent-binary load) is not a drift category and is not in scope for "make tests pass."
- Two `setup_*_pool()` patterns coexist in test files (array vs. verbose). Consolidating them is a clean follow-up hygiene task; not in scope for any given feature phase.
