# TODO — Let's Chat

Current state as of 2026-04-10. Branch: `feat/admin-users-mods-chat`.

## What's Done

All builds run via Docker (no Rust on host):
```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test
```

34 tests passing across 7 test files.

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

### Phase 6: WebSockets (real-time updates)

This is the next phase. No plan has been written yet. Requirements from the design spec (`docs/superpowers/specs/2026-04-10-lets-chat-design.md`, "Real-Time" section):

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

## Reference

- **Design spec:** `docs/superpowers/specs/2026-04-10-lets-chat-design.md`
- **Implementation plans:** `docs/superpowers/plans/2026-04-10-phase*.md` (Phases 1-5)
- **Tech stack:** Dioxus 0.7.3 fullstack, sqlx, Axum 0.8, 3 SQLite databases
- **Build:** `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check`
- **Test:** `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test`
