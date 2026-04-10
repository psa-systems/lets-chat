# Let's Chat — Application Design

App #4 in the a8n Tools suite. A self-hosted chat application built with Dioxus 0.7.x (Rust + WASM), Axum, and SQLite.

## Overview

A self-hosted chat app with user registration, role-based access control (Admin/Moderator/User), an admin panel for site settings and user management, moderation tools, direct messaging, and real-time updates via WebSockets.

## Architecture

**Modular monolith** — single binary, three SQLite databases separated by domain:

- **auth.db** — users, sessions, invite codes
- **chat.db** — rooms, messages, room members, moderation audit log
- **settings.db** — key-value site configuration

Each database gets its own connection pool (`OnceLock<SqlitePool>`), migration folder, and `db/` module. Cross-database references use user IDs (UUIDs) enforced at the application layer, not by foreign keys.

## Data Model

### auth.db

**users**

| Column | Type | Notes |
|---|---|---|
| id | TEXT PK | UUID |
| username | TEXT NOT NULL UNIQUE | |
| display_name | TEXT | |
| password_hash | TEXT NOT NULL | argon2 |
| role | TEXT NOT NULL DEFAULT 'user' | 'admin', 'moderator', 'user' |
| is_banned | BOOLEAN NOT NULL DEFAULT 0 | |
| ban_reason | TEXT | |
| banned_until | TEXT | NULL = permanent, datetime = suspension |
| is_muted | BOOLEAN NOT NULL DEFAULT 0 | |
| muted_until | TEXT | |
| mute_reason | TEXT | |
| created_at | TEXT NOT NULL DEFAULT (datetime('now')) | |
| updated_at | TEXT NOT NULL DEFAULT (datetime('now')) | |

**sessions**

| Column | Type | Notes |
|---|---|---|
| id | TEXT PK | random token |
| user_id | TEXT NOT NULL FK→users | |
| created_at | TEXT NOT NULL DEFAULT (datetime('now')) | |
| expires_at | TEXT NOT NULL | |

**invite_codes**

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| code | TEXT NOT NULL UNIQUE | |
| created_by | TEXT NOT NULL FK→users | |
| used_by | TEXT FK→users | |
| used_at | TEXT | |
| expires_at | TEXT | |
| created_at | TEXT NOT NULL DEFAULT (datetime('now')) | |

### chat.db

**rooms** (existing, extended)

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | existing |
| name | TEXT NOT NULL | existing, UNIQUE constraint only for public rooms (enforced at app layer) |
| topic | TEXT | existing |
| created_by | TEXT | user ID, new |
| room_type | TEXT NOT NULL DEFAULT 'public' | 'public', 'dm', new |
| created_at | TEXT NOT NULL DEFAULT (datetime('now')) | existing |

**messages** (existing, modified)

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | existing |
| room_id | INTEGER NOT NULL FK→rooms | existing |
| user_id | TEXT NOT NULL | replaces `author`, references auth.db |
| body | TEXT NOT NULL | existing |
| deleted_at | TEXT | soft delete, new |
| deleted_by | TEXT | moderator ID, new |
| created_at | TEXT NOT NULL DEFAULT (datetime('now')) | existing |

**room_members** (new)

| Column | Type | Notes |
|---|---|---|
| room_id | INTEGER NOT NULL FK→rooms | |
| user_id | TEXT NOT NULL | |
| joined_at | TEXT NOT NULL DEFAULT (datetime('now')) | |
| PRIMARY KEY | (room_id, user_id) | |

**mod_actions** (new)

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK AUTOINCREMENT | |
| action | TEXT NOT NULL | 'ban', 'mute', 'kick', 'suspend', 'delete_message' |
| target_user | TEXT NOT NULL | |
| actor_user | TEXT NOT NULL | |
| reason | TEXT | |
| room_id | INTEGER | NULL for global actions |
| metadata | TEXT | JSON for extra context |
| created_at | TEXT NOT NULL DEFAULT (datetime('now')) | |

### settings.db

**settings** (key-value store)

| Key | Default | Description |
|---|---|---|
| site_name | 'Let's Chat' | Display name |
| registration_open | 'true' | Allow new registrations |
| require_invite_code | 'false' | Require invite code to register |
| default_role | 'user' | Role assigned to new users |
| welcome_message | 'Welcome to Let's Chat!' | Shown on welcome page |
| max_message_length | '4000' | Character limit per message |
| motd | '' | Message of the day |
| maintenance_mode | 'false' | Block non-admin access |
| rate_limit_messages | '30' | Messages per minute per user |
| smtp_host | '' | SMTP server host |
| smtp_port | '587' | SMTP server port |
| smtp_user | '' | SMTP username |
| smtp_pass | '' | SMTP password (AES-256-GCM encrypted, key from env var `LETS_CHAT_SECRET_KEY`) |

## Authentication

### Registration Flow

1. User submits username + password (+ invite code if required by settings)
2. Server validates: username uniqueness, password minimum 8 characters, invite code validity if required
3. Password hashed with argon2, user created with default role from settings
4. Session token generated, stored in `sessions` table, returned as HTTP-only cookie
5. First registered user is automatically promoted to `admin`

### Login Flow

1. User submits username + password
2. Server verifies argon2 hash, checks ban/suspension status
3. If valid and not banned: create session, set cookie
4. If suspended: check `banned_until` — if expired, clear suspension and allow login

### Session Management

- HTTP-only, Secure, SameSite=Strict cookies
- 30-day expiry, refreshed on activity
- Server functions extract current user from session cookie via Axum middleware
- `use_current_user()` hook on frontend provides logged-in user to components

### Route Protection

- Unauthenticated users see only login/register pages
- All chat routes require a valid session
- Admin routes require `role = 'admin'`
- Mod actions require `role = 'admin'` or `role = 'moderator'`

## RBAC

Three global roles with hierarchical permissions:

| Capability | Admin | Moderator | User |
|---|---|---|---|
| Chat in rooms | yes | yes | yes |
| Send DMs | yes | yes | yes |
| Create/delete rooms | yes | no | no |
| Delete messages | yes | yes | no |
| Mute users | yes | yes | no |
| Kick users | yes | yes | no |
| Ban/suspend users | yes | yes | no |
| Change user roles | yes | no | no |
| Manage site settings | yes | no | no |
| Manage invite codes | yes | no | no |
| View moderation log | yes | yes | no |

## Admin Panel

Accessible via sidebar link, visible only to admins. Uses a tabbed layout within the main content area.

### Tabs

**Site Settings** — Grouped into cards (General, Registration, Limits, SMTP). Edit inline, save per section.

**Users** — Searchable/filterable user list. Click through to detail view with role change + moderation actions.

**Invite Codes** — Generate codes, view usage status (used by whom, when), revoke unused codes.

**Moderation Log** — Chronological audit trail of all mod actions: who did what, to whom, when, and why.

**Rooms** — Create/delete rooms, edit names and topics.

Moderators see a reduced view: Users tab (mod actions only, no role changes) and Moderation Log.

## Moderation

### Actions

- **Mute** — user can't send messages for a specified duration. Composer is disabled with "You are muted until {time}" message.
- **Ban** — user can't log in. Logged out immediately via WebSocket disconnect. Login page shows "Your account has been banned: {reason}".
- **Kick** — user removed from a room and redirected to welcome page.
- **Suspend** — temporary ban with expiry. Login page shows "Suspended until {date}: {reason}". Cleared automatically on login attempt after expiry.
- **Message deletion** — soft delete (sets `deleted_at` and `deleted_by`). Message hidden from view.

### UX

- Moderators and admins see action buttons on messages and usernames
- All actions require a reason and create an entry in `mod_actions`
- WebSocket broadcasts mod events so affected users see changes in real-time

## Direct Messages

- Sidebar shows a "Direct Messages" section below rooms
- DMs implemented as rooms with `room_type = 'dm'` and exactly 2 entries in `room_members`
- Initiating a DM checks for existing DM room between the two users; reuses if found
- DM rooms don't appear in the public room list, only in each participant's sidebar
- Route: `/dm/:user_id` — component resolves the DM room behind the scenes
- Clicking a username in chat offers "Send Direct Message"

## Real-Time (WebSockets)

### Server

- Dedicated `/ws` endpoint on Axum router, outside Dioxus server functions
- On connect: validate session cookie, reject if unauthenticated or banned
- In-memory `HashMap<RoomId, HashSet<UserConnection>>` for room subscriptions
- Message sends (via server function) broadcast to all subscribed connections
- Ping/pong every 30 seconds for stale connection detection

### Events

- `NewMessage` — new message in a subscribed room
- `MessageDeleted` — message soft-deleted by moderator
- `UserJoined` — user entered a room
- `UserLeft` — user left a room
- `UserMuted` — user muted
- `UserBanned` — user banned (triggers connection close for target)
- `UserKicked` — user kicked from room

### Client

- `use_websocket()` hook establishes connection on login, reconnects with exponential backoff
- Provides `Signal<Vec<ChatEvent>>` for components to subscribe to
- `RoomViewPage` appends new messages from WebSocket — no full refetch
- `Sidebar` listens for room list changes
- DM notifications come through the same connection

### Message Flow

Sending stays as HTTP server functions (proper error handling). WebSocket is server-to-client only for event fan-out. Client sends `Subscribe { room_id }` / `Unsubscribe { room_id }` control frames as user navigates between rooms.

## Routes

### Public

| Path | Component | Description |
|---|---|---|
| /login | LoginPage | Username + password login |
| /register | RegisterPage | New user registration |

### Authenticated — Chat Layout

| Path | Component | Description |
|---|---|---|
| / | WelcomePage | Landing page with welcome message |
| /room/:room_id | RoomViewPage | Chat room view |
| /dm/:user_id | DmViewPage | Direct message view |

### Authenticated — Admin Layout

| Path | Component | Description |
|---|---|---|
| /admin | AdminSettingsPage | Site settings |
| /admin/users | AdminUsersPage | User list |
| /admin/users/:id | AdminUserDetailPage | User detail + mod actions |
| /admin/invites | AdminInvitesPage | Invite code management |
| /admin/modlog | AdminModLogPage | Moderation audit log |
| /admin/rooms | AdminRoomsPage | Room management |

## Module Structure

```
src/
├── main.rs
├── routes.rs
├── models/
│   ├── mod.rs
│   ├── message.rs          (existing, modified: author → user_id, add deleted_at/deleted_by)
│   ├── room.rs             (existing, extended: add created_by, room_type)
│   ├── user.rs             NEW
│   ├── session.rs          NEW
│   ├── invite.rs           NEW
│   ├── mod_action.rs       NEW
│   └── settings.rs         NEW
├── db/
│   ├── mod.rs              (extended: auth + settings pools)
│   ├── chat.rs             (existing, extended)
│   ├── auth.rs             NEW
│   └── settings.rs         NEW
├── server_fns/
│   ├── mod.rs
│   ├── chat.rs             (existing, extended)
│   ├── auth.rs             NEW
│   ├── admin.rs            NEW
│   └── moderation.rs       NEW
├── components/
│   ├── mod.rs
│   ├── auth_layout.rs      NEW — session check, user context, WebSocket
│   ├── layout.rs           (renamed to ChatLayout, extended: DMs + admin link)
│   ├── sidebar.rs          (existing, extended)
│   ├── room_view.rs        (existing, enhanced with WebSocket + user identity)
│   ├── welcome.rs          (existing)
│   ├── dm_view.rs          NEW
│   ├── login.rs            NEW
│   ├── register.rs         NEW
│   ├── admin/
│   │   ├── mod.rs          NEW
│   │   ├── layout.rs       NEW — tab bar
│   │   ├── settings.rs     NEW
│   │   ├── users.rs        NEW
│   │   ├── user_detail.rs  NEW
│   │   ├── invites.rs      NEW
│   │   ├── mod_log.rs      NEW
│   │   └── rooms.rs        NEW
│   └── hooks/
│       ├── mod.rs          NEW
│       ├── use_current_user.rs  NEW
│       └── use_websocket.rs     NEW
├── ws/                     NEW — WebSocket handler
│   ├── mod.rs
│   ├── handler.rs          — upgrade, auth, connection loop
│   └── hub.rs              — room subscriptions, broadcast
migrations/
├── auth/
│   └── 0001_create_tables.sql   NEW
├── chat/
│   ├── 0001_create_tables.sql   (existing)
│   └── 0002_add_users_dms_moderation.sql  NEW
└── settings/
    └── 0001_create_tables.sql   NEW
```

## Dependencies

New crate dependencies needed:

| Crate | Purpose |
|---|---|
| argon2 | Password hashing |
| rand | Session token + invite code generation |
| tokio-tungstenite | WebSocket support |
| futures-util | Stream utilities for WebSocket |

## Build Order

These are built and integrated incrementally:

1. **Auth** — users, sessions, login/register, `use_current_user` hook, route protection
2. **RBAC** — role checks in server functions, permission-gated UI
3. **Admin panel** — site settings, user management, invite codes, room management
4. **Moderation** — mod actions, `mod_actions` audit log, mod UX on messages/users
5. **Chat enhancements** — DMs (room_members, dm_view, sidebar DM list)
6. **WebSockets** — ws handler, hub, `use_websocket` hook, real-time message delivery

Each phase produces a working application — no phase depends on later phases.
