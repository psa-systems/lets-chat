# TODO — Let's Chat

Current state as of 2026-04-10. Branch: `feat/admin-users-mods-chat`.

## Getting Started (read this first)

**There is no Rust toolchain on the host machine.** All compilation and testing MUST use Docker:
```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test
```

34 tests passing across 7 test files. Run `cargo test` to verify before starting work.

### Tech Stack
- **Dioxus 0.7.3** fullstack (Rust + WASM + Axum 0.8 on server)
- **sqlx 0.8** with 3 separate **SQLite** databases: auth.db, chat.db, settings.db
- **Tailwind CSS** for styling (pre-built CSS is a gitignored build artifact — `asset!("/assets/tailwind-built.css")` will fail at compile time but this is pre-existing and doesn't affect `cargo check` or tests)

### Project Structure
```
src/
├── main.rs              # Entry point, pool init, server router setup
├── lib.rs               # Library target for integration tests (pub mod db, models, server_fns)
├── routes.rs            # Dioxus router enum + route components
├── db/
│   ├── mod.rs           # Pool initialization (OnceCell pattern), migration runner
│   ├── auth.rs          # User CRUD, sessions, ban/mute/invite functions
│   ├── chat.rs          # Rooms, messages, DM functions
│   ├── moderation.rs    # mod_actions audit log, soft-delete
│   └── settings.rs      # Key-value settings store
├── models/
│   ├── mod.rs           # Re-exports all models
│   ├── user.rs          # UserRecord (server-only) + User (public, serde)
│   ├── message.rs       # Message struct
│   ├── room.rs          # Room struct (has room_type field)
│   ├── invite.rs        # InviteCode struct
│   ├── mod_action.rs    # ModAction struct
│   └── settings.rs      # SiteSettings struct
├── server_fns/
│   ├── mod.rs           # Module registration
│   ├── auth.rs          # register, login, logout, get_current_user
│   ├── chat.rs          # list_rooms, get_room, list_messages, send_message
│   ├── admin.rs         # 11 admin endpoints (require admin, except list_users requires moderator)
│   ├── moderation.rs    # ban, mute, suspend, kick, delete_message, etc.
│   ├── dm.rs            # get_or_create_dm, list_my_dms, send_dm_message
│   └── helpers.rs       # require_auth(), require_role(), role_level() — SERVER-ONLY (#[cfg(feature = "server")])
└── components/
    ├── mod.rs
    ├── auth_layout.rs   # Session gate, provides Signal<User> context
    ├── layout.rs        # Sidebar + Outlet
    ├── sidebar.rs       # Room list, DM list, admin/moderate link, user info
    ├── login.rs / register.rs / welcome.rs
    ├── room_view.rs     # Chat room with mod actions + DM links on usernames
    ├── dm_view.rs       # DM conversation view
    └── admin/
        ├── layout.rs    # Tab bar (role-aware: admins see all, mods see Users + Mod Log)
        ├── settings.rs / users.rs / invites.rs / rooms.rs / mod_log.rs
migrations/
├── auth/0001_create_tables.sql
├── chat/0001_create_tables.sql
├── chat/0002_moderation.sql
├── chat/0003_dms.sql
└── settings/0001_create_tables.sql
tests/
├── db_auth.rs / rbac.rs / db_settings.rs / db_invite.rs / db_moderation.rs / db_dm.rs
```

### Key Patterns
- **Server functions** use `#[server]` macro. They call `require_auth()` or `require_role("moderator")` for auth.
- **DB modules** are gated with `#[cfg(not(target_arch = "wasm32"))]` since they can't compile to WASM.
- **helpers.rs** is gated with `#[cfg(feature = "server")]` in mod.rs since it uses `extract()` from axum.
- **Dioxus 0.7 router** uses stacking `#[layout(X)]` attributes (no `#[end_layout]` needed).
- **Signal<User>** is provided via `use_context_provider` in `AuthLayout` and consumed via `use_context::<Signal<User>>()` in child components.
- **Cross-DB resolution**: messages store `user_id` (from auth.db) in chat.db. The server function layer resolves display names by looking up auth.db.

## What's Done

### Phase 1: Auth ✅
- SQLite databases: auth.db, chat.db, settings.db (separate pools, lazy-initialized)
- User registration + login with argon2 password hashing
- Session cookies (HTTP-only, SameSite=Lax, 30-day expiry)
- First registered user auto-promoted to admin
- AuthLayout gate redirects unauthenticated users to /login
- Login + Register pages with validation

### Phase 2: RBAC ✅
- `require_auth()` / `require_role()` server-side helpers in `src/server_fns/helpers.rs`
- Role hierarchy: admin (3) > moderator (2) > user (1)
- Admin link in sidebar (visible to admin/moderator)

### Phase 3: Admin Panel ✅
- Settings page (General, Registration, Limits, SMTP sections)
- User management (role dropdown, delete)
- Invite code generation/revocation
- Room management (create, edit, delete)
- 11 admin server functions, all require admin role

### Phase 4: Moderation ✅
- `mod_actions` audit table in chat.db
- Soft-delete on messages (`deleted_at`, `deleted_by`)
- Ban, unban, suspend, mute, unmute, kick DB + server functions
- Role hierarchy enforcement (can't moderate equal/higher role)
- Session invalidation on ban/suspend
- Mod Log admin page with color-coded action badges
- Moderators see Users + Mod Log tabs (admins see all tabs)
- Ban/Mute/Unban/Unmute buttons on admin users page with reason/duration modals
- Message delete button (hover-reveal) for moderators in room view
- Mute banner replaces composer when user is muted

### Phase 5: Direct Messages ✅
- `room_members` table, `room_type`/`created_by` columns on rooms
- DM rooms (`room_type = 'dm'`) with exactly 2 members
- `find_dm_room` / `create_dm_room` / `list_user_dm_rooms` DB functions
- `get_or_create_dm` reuses existing DM room between two users
- `/dm/:user_id` route with DM view component
- Sidebar "Direct Messages" section with @username links
- Clickable usernames in room view link to DM
- `list_rooms` filters to public rooms only

---

## What's Left

### Phase 6: WebSockets (real-time updates) — START HERE

This is the next phase. No plan has been written yet. To begin:
1. Read the design spec: `docs/superpowers/specs/2026-04-10-lets-chat-design.md` (especially the "Real-Time (WebSockets)" section starting around line 221)
2. Look at existing Phase plans in `docs/superpowers/plans/` for the format used
3. Read `src/main.rs` to understand how the Axum router is set up (the `/ws` endpoint needs to be registered there)
4. Read `src/server_fns/chat.rs` (send_message) and `src/server_fns/moderation.rs` — these are where broadcast calls need to be added

Requirements from the design spec:

**Server side:**
- Dedicated `/ws` endpoint on Axum router, outside Dioxus server functions
- On connect: validate session cookie, reject if unauthenticated or banned
- In-memory `HashMap<RoomId, HashSet<UserConnection>>` for room subscriptions
- Message sends (via server function) broadcast to all subscribed connections
- Ping/pong every 30 seconds for stale connection detection

**Events:**
- `NewMessage` — new message in a subscribed room
- `MessageDeleted` — message soft-deleted by moderator
- `UserJoined` / `UserLeft` — user entered/left a room
- `UserMuted` / `UserBanned` / `UserKicked` — mod actions

**Client side:**
- `use_websocket()` hook: connects on login, reconnects with exponential backoff
- Provides `Signal<Vec<ChatEvent>>` for components to subscribe to
- `RoomViewPage` appends new messages from WebSocket (no full refetch)
- Sidebar listens for room list changes
- DM notifications through the same connection
- Client sends `Subscribe { room_id }` / `Unsubscribe { room_id }` control frames as user navigates rooms

**Architecture note:** Sending stays as HTTP server functions (proper error handling). WebSocket is server-to-client only for event fan-out.

**Key files to create/modify:**
- New: WebSocket hub module (in-memory connection registry)
- New: `/ws` endpoint handler in Axum router
- New: `use_websocket()` client hook
- New: Event types (serde-serializable enum)
- Modify: `src/main.rs` — register `/ws` route on the Axum router
- Modify: `src/server_fns/chat.rs` — broadcast `NewMessage` after `insert_message`
- Modify: `src/server_fns/moderation.rs` — broadcast mod events after actions
- Modify: `src/components/room_view.rs` — subscribe to WebSocket, append messages
- Modify: `src/components/sidebar.rs` — listen for updates

**Dependencies to add:** `tokio-tungstenite` or `axum`'s built-in WebSocket support (`axum::extract::ws`), `futures` for stream handling.

### Future Enhancements (see FUTURE.md)

These are deferred and not part of the current build plan:
- Private/invite-only rooms
- File/image uploads
- Message editing
- Reactions/emoji
- Typing indicators
- Read receipts
- Message search

---

## Known Gotchas

- **No Rust on host** — every `cargo` command must go through Docker. See top of file.
- **Tailwind CSS** — `asset!("/assets/tailwind-built.css")` in main.rs references a gitignored build artifact. This causes a compile error for the full binary but does NOT affect `cargo check` or `cargo test`. This is pre-existing.
- **helpers.rs cfg gate** — `src/server_fns/helpers.rs` is gated with `#[cfg(feature = "server")]` in mod.rs, not `#[cfg(not(target_arch = "wasm32"))]`. This is because it uses `dioxus::prelude::extract()` which requires the server feature.
- **Dioxus 0.7 router** — uses `#[layout(X)]` attribute stacking. There is no `#[end_layout]`. Multiple `#[layout()]` attributes nest.
- **Integration tests** — use `src/lib.rs` as the library target. The lib exposes `pub mod db`, `pub mod models`, `pub mod server_fns`. Tests create in-memory SQLite pools and run all migrations manually.

## Reference

- **Design spec:** `docs/superpowers/specs/2026-04-10-lets-chat-design.md` — the authoritative source for all requirements
- **Implementation plans:** `docs/superpowers/plans/2026-04-10-phase*.md` (Phases 1, 2, 4, 5 have written plans)
- **Build:** `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check`
- **Test:** `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test`
- **Cargo.toml dependencies:** dioxus 0.7.3, sqlx 0.8, axum 0.8, argon2 0.5, chrono, serde, serde_json, uuid, rand 0.8, tokio
