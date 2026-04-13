# Phase 9: Private / Invite-Only Rooms Implementation Plan

**Goal:** Rooms that only members can see or post in. Architecturally foundational — gates access for search (Phase 11) and file uploads (Phase 13).

**Architecture:** Reuse the existing `room_members` table (already used for DMs). Add `invite_code TEXT UNIQUE` to `rooms`. `list_rooms` filters by role: admins see everything, regular users see public rooms plus private rooms they joined. Access checks are added to `list_messages` and `send_message`. New server functions handle joining by invite link and leaving a room. Admins can invite users directly by username and regenerate invite codes.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/chat/0005_private_rooms.sql` | Add `invite_code` column and index |
| Modify | `src/models/room.rs` | Add `invite_code: Option<String>` |
| Modify | `src/db/chat.rs` | Update `list_rooms`, `get_room`, `create_room`; add `add_room_member`, `remove_room_member`, `get_room_by_invite`, `is_room_member`, `regenerate_invite_code` |
| Modify | `src/db/mod.rs` | Run migration 005 |
| Modify | `src/ws/events.rs` | Add `RoomMemberAdded`, `RoomMemberRemoved` variants |
| Modify | `src/server_fns/chat.rs` | Update `list_rooms`, `list_messages`, `send_message` with access checks |
| Modify | `src/server_fns/admin.rs` | Update `create_room` (add `room_type`), add `invite_user_to_room`, `regenerate_invite_code` |
| Create | `src/server_fns/rooms.rs` | `join_room_by_invite`, `leave_room` |
| Modify | `src/server_fns/mod.rs` | Declare `rooms` module |
| Modify | `src/routes.rs` | Add `/invite/:code` route |
| Create | `src/components/invite.rs` | Invite landing page |
| Modify | `src/components/mod.rs` | Declare `invite` module |
| Modify | `src/components/admin/rooms.rs` | Add `room_type` dropdown, invite link, invite-user form |
| Modify | `src/components/sidebar.rs` | Bump rooms on `RoomMemberAdded/Removed` WS events |
| Create | `tests/db_private_rooms.rs` | Integration tests |

---

### Task 1: DB Migration

**File:** `migrations/chat/0005_private_rooms.sql`

- [x] **Step 1: Write migration**

```sql
ALTER TABLE rooms ADD COLUMN invite_code TEXT UNIQUE;
CREATE INDEX IF NOT EXISTS idx_rooms_invite_code ON rooms(invite_code);
```

- [x] **Step 2: Register in `src/db/mod.rs`**

In `init_chat_pool`, after the migration 004 block add:

```rust
let migration_005 = include_str!("../../migrations/chat/0005_private_rooms.sql");
sqlx::raw_sql(migration_005).execute(&pool).await.expect("Failed to run chat DB migration 005");
```

---

### Task 2: Room model

**File:** `src/models/room.rs`

- [x] **Step 1: Add `invite_code` field**

```rust
pub struct Room {
    pub id: i64,
    pub name: String,
    pub topic: Option<String>,
    pub room_type: String,
    pub invite_code: Option<String>,
    pub created_at: String,
}
```

Fix every Room construction site to include `invite_code: row.get("invite_code")` (or `invite_code: None` for DM constructions that don't need it).

---

### Task 3: DB layer

**File:** `src/db/chat.rs`

- [x] **Step 1: Update `list_rooms` — user-scoped, exclude DMs**

Replace the existing `list_rooms` signature and query:

```rust
pub async fn list_rooms(pool: &sqlx::SqlitePool, user_id: &str, is_admin: bool) -> Result<Vec<Room>, sqlx::Error>
```

Admin path (sees all non-DM rooms):
```sql
SELECT id, name, topic, room_type, invite_code, created_at
FROM rooms WHERE room_type != 'dm' ORDER BY name
```

Non-admin path (public rooms + private rooms where user is a member):
```sql
SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at
FROM rooms r
LEFT JOIN room_members m ON m.room_id = r.id AND m.user_id = ?
WHERE r.room_type != 'dm' AND (r.room_type = 'public' OR m.user_id IS NOT NULL)
ORDER BY r.name
```

- [x] **Step 2: Update `get_room` to include `invite_code`**

Add `invite_code` to the SELECT and Room mapping.

- [x] **Step 3: Update `create_room` to accept `room_type` and `invite_code`**

```rust
pub async fn create_room(
    pool: &sqlx::SqlitePool,
    name: &str,
    topic: Option<&str>,
    room_type: &str,
    invite_code: Option<&str>,
) -> Result<i64, sqlx::Error>
```

```sql
INSERT INTO rooms (name, topic, room_type, invite_code) VALUES (?, ?, ?, ?)
```

- [x] **Step 4: Add helper functions**

```rust
/// Check if a user is a member of a room.
pub async fn is_room_member(pool: &sqlx::SqlitePool, room_id: i64, user_id: &str) -> Result<bool, sqlx::Error>

/// Add a user to room_members.
pub async fn add_room_member(pool: &sqlx::SqlitePool, room_id: i64, user_id: &str) -> Result<(), sqlx::Error>

/// Remove a user from room_members.
pub async fn remove_room_member(pool: &sqlx::SqlitePool, room_id: i64, user_id: &str) -> Result<(), sqlx::Error>

/// Find a room by its invite code.
pub async fn get_room_by_invite(pool: &sqlx::SqlitePool, invite_code: &str) -> Result<Option<Room>, sqlx::Error>

/// Set a new invite code for a room. Returns the new code.
pub async fn regenerate_invite_code(pool: &sqlx::SqlitePool, room_id: i64, new_code: &str) -> Result<(), sqlx::Error>
```

- [x] **Step 5: Fix DM-related Room constructions**

`find_dm_room`, `create_dm_room`, `list_user_dm_rooms` all construct `Room` inline — add `invite_code: None` to each.

---

### Task 4: WS Events

**File:** `src/ws/events.rs`

- [x] **Step 1: Add member events to `ChatEvent`**

```rust
RoomMemberAdded { room_id: i64, user_id: String },
RoomMemberRemoved { room_id: i64, user_id: String },
```

---

### Task 5: Server functions — update existing

**File:** `src/server_fns/chat.rs`

- [x] **Step 1: Update `list_rooms`**

```rust
#[server]
pub async fn list_rooms() -> Result<Vec<Room>, ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let is_admin = crate::server_fns::helpers::role_level(&user.role) >= 3;
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::list_rooms(pool, &user.id, is_admin)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
```

- [x] **Step 2: Add `require_room_access` helper (private to the module)**

```rust
/// Returns Err if the user cannot access the room.
/// Public rooms: always OK. Private/DM rooms: must be a member. Admins bypass all checks.
async fn require_room_access(pool: &sqlx::SqlitePool, room_id: i64, user_id: &str, is_admin: bool) -> Result<(), ServerFnError> {
    if is_admin {
        return Ok(());
    }
    let room = crate::db::chat::get_room(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Room not found"))?;
    if room.room_type == "public" {
        return Ok(());
    }
    let member = crate::db::chat::is_room_member(pool, room_id, user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if member {
        Ok(())
    } else {
        Err(ServerFnError::new("Access denied"))
    }
}
```

- [x] **Step 3: Add access check to `list_messages`**

At the top of `list_messages`, before the DB query:

```rust
let user = crate::server_fns::helpers::require_auth().await?;
let is_admin = crate::server_fns::helpers::role_level(&user.role) >= 3;
require_room_access(chat_pool, room_id, &user.id, is_admin).await?;
```

- [x] **Step 4: Add access check to `send_message`**

After the mute check, before `insert_message`:

```rust
let is_admin = crate::server_fns::helpers::role_level(&user.role) >= 3;
let chat_pool = crate::db::get_chat_pool().await;
require_room_access(chat_pool, room_id, &user.id, is_admin).await?;
```

---

### Task 6: Admin server functions

**File:** `src/server_fns/admin.rs`

- [x] **Step 1: Update `create_room` to accept `room_type`**

```rust
#[server]
pub async fn create_room(name: String, topic: String, room_type: String) -> Result<Room, ServerFnError>
```

Generate invite_code if `room_type == "private"`:

```rust
use rand::Rng;
let invite_code: Option<String> = if room_type == "private" {
    Some(rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect())
} else {
    None
};
```

Pass both to `db::chat::create_room(pool, &name, topic_opt, &room_type, invite_code.as_deref())`.

- [x] **Step 2: Add `invite_user_to_room`**

```rust
#[server]
pub async fn invite_user_to_room(room_id: i64, username: String) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("moderator").await?;
    // Look up target user by username in auth DB
    // Add to room_members
    // Broadcast RoomMemberAdded
}
```

- [x] **Step 3: Add `regenerate_invite_code`**

```rust
#[server]
pub async fn regenerate_invite_code(room_id: i64) -> Result<String, ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;
    // Generate new random 16-char code
    // Update DB
    // Return new code
}
```

---

### Task 7: New server functions — rooms

**File:** `src/server_fns/rooms.rs`

- [x] **Step 1: Create the file with `join_room_by_invite` and `leave_room`**

```rust
#[server]
pub async fn join_room_by_invite(invite_code: String) -> Result<i64, ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;
    let room = crate::db::chat::get_room_by_invite(pool, &invite_code)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Invalid invite code"))?;
    if room.room_type != "private" {
        return Err(ServerFnError::new("Invalid invite code"));
    }
    let already = crate::db::chat::is_room_member(pool, room.id, &user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !already {
        crate::db::chat::add_room_member(pool, room.id, &user.id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let event = crate::ws::events::ChatEvent::RoomMemberAdded { room_id: room.id, user_id: user.id.clone() };
        crate::ws::hub::get_hub().broadcast_global(&event);
    }
    Ok(room.id)
}

#[server]
pub async fn leave_room(room_id: i64) -> Result<(), ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::remove_room_member(pool, room_id, &user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let event = crate::ws::events::ChatEvent::RoomMemberRemoved { room_id, user_id: user.id.clone() };
    crate::ws::hub::get_hub().broadcast_global(&event);
    Ok(())
}
```

- [x] **Step 2: Declare in `src/server_fns/mod.rs`**

---

### Task 8: Routes — invite page

**Files:** `src/routes.rs`, `src/components/invite.rs`, `src/components/mod.rs`

- [x] **Step 1: Add route to `routes.rs`**

```rust
#[route("/invite/:code")]
Invite { code: String },
```

Add the corresponding component fn and import.

- [x] **Step 2: Create `src/components/invite.rs`**

The page calls `join_room_by_invite(code)` on mount. On success, navigates to `/room/:room_id`. On error, shows a message.

```rust
#[component]
pub fn InvitePage(code: String) -> Element {
    let nav = use_navigator();
    let result = use_server_future(move || {
        let c = code.clone();
        async move { join_room_by_invite(c).await }
    })?;
    match result() {
        Some(Ok(room_id)) => {
            nav.push(Route::Room { room_id: room_id.to_string() });
            rsx! { div { "Joining room…" } }
        }
        Some(Err(e)) => rsx! {
            div { class: "flex-1 flex items-center justify-center",
                div { class: "text-center",
                    p { class: "text-red-600 mb-4", "Could not join: {e}" }
                    Link { to: Route::Home {}, class: "text-blue-600 hover:underline", "Go home" }
                }
            }
        },
        None => rsx! { div { class: "flex-1 flex items-center justify-center text-gray-500", "Joining…" } },
    }
}
```

---

### Task 9: UI — Admin rooms page

**File:** `src/components/admin/rooms.rs`

- [x] **Step 1: Add `room_type` signal and dropdown to create form**

Add `let mut new_room_type = use_signal(|| "public".to_string())` and a `<select>` field with options `public` / `private`.

- [x] **Step 2: Pass `room_type` to `create_room`**

Update the `create_room(name, topic)` call to `create_room(name, topic, room_type)`.

- [x] **Step 3: Show invite link for private rooms in the table**

In the non-editing row, if `room.room_type == "private"` and `invite_code.is_some()`, show a copy-able invite URL: `https://{host}/invite/{code}`. For simplicity, render the path `/invite/{code}` and an "Invite user" button that opens an inline input.

- [x] **Step 4: Add "Invite user" inline form per private room**

Add signals `inviting_room: Signal<Option<i64>>` and `invite_username: Signal<String>`. When "Invite" is clicked for a private room, show a small input + Submit button that calls `invite_user_to_room(room_id, username)`.

- [x] **Step 5: Add "Regenerate invite link" button per private room**

Calls `regenerate_invite_code(room_id)` and restarts the rooms future.

---

### Task 10: UI — Sidebar rooms refresh

**File:** `src/components/sidebar.rs`

- [x] **Step 1: Add `rooms_version` signal and bump on member events**

Mirror the existing `dms_version` pattern:

```rust
let mut rooms_version = use_signal(|| 0u32);
let rooms = use_server_future(move || {
    let _v = rooms_version();
    async move { list_rooms().await }
})?;
```

Add a `use_effect` that bumps `rooms_version` on `RoomMemberAdded` and `RoomMemberRemoved` events for the current user.

---

### Task 11: Integration tests

**File:** `tests/db_private_rooms.rs`

- [x] **Step 1: Write tests**

- Non-member cannot call `list_messages` on a private room → `Access denied` error
- Member can access after `join_room_by_invite`
- Invalid invite code returns error
- `list_rooms` for non-member excludes the private room
- Admin can invite a user directly via `invite_user_to_room`

---

## Phase 9 complete checklist

- [x] Migration 005 adds `invite_code TEXT UNIQUE` to rooms
- [x] `Room` model has `invite_code: Option<String>`
- [x] `list_rooms` filters by role — admins see all, users see public + joined private
- [x] `list_messages` and `send_message` reject non-members of private rooms
- [x] `join_room_by_invite` inserts into `room_members`, broadcasts `RoomMemberAdded`
- [x] `leave_room` deletes from `room_members`, broadcasts `RoomMemberRemoved`
- [x] `invite_user_to_room` (mod+) adds a user by username
- [x] `regenerate_invite_code` (admin) rotates the invite code
- [x] Admin rooms UI shows `room_type` dropdown, invite link, invite-user form
- [x] `/invite/:code` route joins and redirects
- [x] Sidebar refreshes room list on member events
- [x] Integration tests pass
