# Read Receipts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add DM read receipts with "Seen" labels, sidebar unread badges, and a per-user symmetric privacy toggle.

**Architecture:** High-water mark per (user, DM room) in `chat.db`. A per-user boolean in `auth.db` gates both sending and receiving receipts symmetrically. WebSocket event `DmRead` pushes updates in real time. Client marks read on mount + tab-visible + new peer messages.

**Tech Stack:** SQLx + SQLite, Axum, Dioxus 0.7 (Signals, use_effect), `web-sys` for `visibilitychange`, `tokio` test harness.

**Spec:** `docs/superpowers/specs/2026-04-13-read-receipts-design.md`

---

## File Structure

**Create:**
- `migrations/chat/0006_read_receipts.sql` — `dm_read_state` table
- `migrations/auth/0002_read_receipts.sql` — `read_receipts_enabled` column
- `tests/db_read_receipts.rs` — integration tests

**Modify:**
- `src/db/mod.rs` — register new migrations in `init_chat_pool` / `init_auth_pool`
- `src/db/chat.rs` — add `upsert_dm_read`, `get_dm_read_state`, `list_dm_unread_counts`
- `src/db/auth.rs` — add `set_read_receipts_enabled`, update `SELECT`s to include column
- `src/models/user.rs` — add `read_receipts_enabled: bool` to `UserRecord` and `User`
- `src/server_fns/dm.rs` — add `mark_dm_read`, `list_dm_unread_counts_fn`, `get_dm_peer_read_state`; update `User` mappings
- `src/server_fns/auth.rs` — add `set_read_receipts_enabled` server fn, update `me`/login mappings
- `src/server_fns/helpers.rs` — nothing structural
- `src/ws/events.rs` — add `DmRead` variant
- `src/components/dm_view.rs` — visibility-gated mark-read, "Seen" label, `DmRead` handler
- `src/components/sidebar.rs` — unread badges
- `src/components/layout.rs` (or new `settings.rs`) — toggle UI for read receipts

---

## Task 1: Chat migration — `dm_read_state` table

**Files:**
- Create: `migrations/chat/0006_read_receipts.sql`
- Modify: `src/db/mod.rs`
- Modify: `tests/db_dm.rs` (register new migration in `setup_pools`) — also other test helpers

- [ ] **Step 1: Write the migration SQL**

Create `migrations/chat/0006_read_receipts.sql`:

```sql
CREATE TABLE IF NOT EXISTS dm_read_state (
    user_id              TEXT    NOT NULL,
    room_id              INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    last_read_message_id INTEGER NOT NULL,
    updated_at           TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, room_id)
);
CREATE INDEX IF NOT EXISTS idx_dm_read_state_room ON dm_read_state(room_id);
```

- [ ] **Step 2: Register migration at startup**

In `src/db/mod.rs`, inside `init_chat_pool`, after the migration_005 block, add:

```rust
let migration_006 = include_str!("../../migrations/chat/0006_read_receipts.sql");
sqlx::raw_sql(migration_006)
    .execute(&pool)
    .await
    .expect("Failed to run chat DB migration 006");
```

- [ ] **Step 3: Register migration in every test helper that builds a chat pool**

Every file under `tests/` that calls `include_str!("../migrations/chat/...")` also needs migration 006. Update `tests/db_dm.rs`, `tests/db_private_rooms.rs`, `tests/message_editing.rs`, `tests/db_moderation.rs`, `tests/db_auth.rs`, `tests/rbac.rs`, `tests/db_invite.rs`, `tests/db_settings.rs` (only those that seed a chat pool — check each by grepping `migrations/chat`). For each, add after the migration 005 line:

```rust
let chat_m6 = include_str!("../migrations/chat/0006_read_receipts.sql");
sqlx::raw_sql(chat_m6).execute(&chat_pool).await.expect("chat migration 6");
```

- [ ] **Step 4: Run existing tests to verify nothing broke**

Run: `docker compose run --rm dev cargo test --all-targets`
Expected: all existing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add migrations/chat/0006_read_receipts.sql src/db/mod.rs tests/
git commit -m "feat(migrations): add dm_read_state for read receipts"
```

---

## Task 2: Auth migration — `read_receipts_enabled` column

**Files:**
- Create: `migrations/auth/0002_read_receipts.sql`
- Modify: `src/db/mod.rs`
- Modify: `src/models/user.rs`
- Modify: `src/db/auth.rs`
- Modify: test helpers that build an auth pool

- [ ] **Step 1: Write the migration SQL**

Create `migrations/auth/0002_read_receipts.sql`:

```sql
ALTER TABLE users ADD COLUMN read_receipts_enabled INTEGER NOT NULL DEFAULT 1;
```

- [ ] **Step 2: Register the migration at startup**

In `src/db/mod.rs`, inside `init_auth_pool`, after the first migration block, add:

```rust
let auth_m2 = include_str!("../../migrations/auth/0002_read_receipts.sql");
sqlx::raw_sql(auth_m2)
    .execute(&pool)
    .await
    .expect("Failed to run auth DB migration 002");
```

- [ ] **Step 3: Extend `UserRecord` and `User` models**

In `src/models/user.rs`, add `pub read_receipts_enabled: bool,` as the last field on both `UserRecord` (under `#[cfg(not(target_arch = "wasm32"))]`) and `User`.

- [ ] **Step 4: Extend every `UserRecord` mapping in `src/db/auth.rs`**

In `find_user_by_username`, `find_user_by_id`, `get_user_by_session`, `list_users`: add `read_receipts_enabled` to the SELECT column list (after `updated_at`) and to each `UserRecord { ... }` struct literal:

```rust
// Example: add " read_receipts_enabled" to the SELECT and this line to the struct:
read_receipts_enabled: r.get("read_receipts_enabled"),
```

- [ ] **Step 5: Add `set_read_receipts_enabled` db function**

Append to `src/db/auth.rs`:

```rust
pub async fn set_read_receipts_enabled(
    pool: &SqlitePool,
    user_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET read_receipts_enabled = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(enabled as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 6: Update every `UserRecord -> User` mapping call site**

Grep for `User {` literals that map a `UserRecord`. Add `read_receipts_enabled: r.read_receipts_enabled,` (or `record.read_receipts_enabled`) to each. Known sites:
- `src/server_fns/auth.rs` (login, register, me)
- `src/server_fns/dm.rs::list_users_for_dm`
- `src/server_fns/admin.rs` (any user-listing)
- any other `User { id:` construction the compiler flags

- [ ] **Step 7: Register auth migration in every test helper that builds an auth pool**

Same pattern as chat migration — every `tests/*.rs` file that seeds an auth pool needs:

```rust
let auth_m2 = include_str!("../migrations/auth/0002_read_receipts.sql");
sqlx::raw_sql(auth_m2).execute(&auth_pool).await.expect("auth migration 2");
```

- [ ] **Step 8: Build and run tests**

Run: `docker compose run --rm dev cargo build --all-targets && docker compose run --rm dev cargo test --all-targets`
Expected: clean build, all existing tests pass.

- [ ] **Step 9: Commit**

```bash
git add migrations/auth/0002_read_receipts.sql src/ tests/
git commit -m "feat(auth): add read_receipts_enabled user column"
```

---

## Task 3: `db::chat` — read-state queries (TDD)

**Files:**
- Create: `tests/db_read_receipts.rs`
- Modify: `src/db/chat.rs`

- [ ] **Step 1: Write the failing test file**

Create `tests/db_read_receipts.rs`:

```rust
use sqlx::SqlitePool;

async fn setup_chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/chat/0001_create_tables.sql"),
        include_str!("../migrations/chat/0002_moderation.sql"),
        include_str!("../migrations/chat/0003_dms.sql"),
        include_str!("../migrations/chat/0004_message_editing.sql"),
        include_str!("../migrations/chat/0005_private_rooms.sql"),
        include_str!("../migrations/chat/0006_read_receipts.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn upsert_is_monotonic() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await.unwrap();
    let m1 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "hi").await.unwrap();
    let m2 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "again").await.unwrap();

    lets_chat::db::chat::upsert_dm_read(&pool, "user-a", room.id, m2).await.unwrap();
    // older id should NOT move the watermark backward
    lets_chat::db::chat::upsert_dm_read(&pool, "user-a", room.id, m1).await.unwrap();

    let state = lets_chat::db::chat::get_dm_read_state(&pool, "user-a", room.id)
        .await.unwrap().expect("state");
    assert_eq!(state.last_read_message_id, m2);
}

#[tokio::test]
async fn unread_counts_peer_only_above_watermark() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await.unwrap();
    // 2 peer messages
    let _m1 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "1").await.unwrap();
    let m2 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "2").await.unwrap();
    // own message should not count toward own unread
    let _m3 = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "own").await.unwrap();

    // user-a has not read anything
    let counts = lets_chat::db::chat::list_dm_unread_counts(&pool, "user-a").await.unwrap();
    let got = counts.iter().find(|(r, _)| *r == room.id).map(|(_, c)| *c).unwrap_or(0);
    assert_eq!(got, 2);

    // After reading m2, unread goes to 0
    lets_chat::db::chat::upsert_dm_read(&pool, "user-a", room.id, m2).await.unwrap();
    let counts = lets_chat::db::chat::list_dm_unread_counts(&pool, "user-a").await.unwrap();
    let got = counts.iter().find(|(r, _)| *r == room.id).map(|(_, c)| *c).unwrap_or(0);
    assert_eq!(got, 0);
}

#[tokio::test]
async fn unread_counts_only_dms_user_is_in() {
    let pool = setup_chat_pool().await;
    // DM user-b <-> user-c (user-a not a member)
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-b-c", "user-b", "user-c")
        .await.unwrap();
    lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "hi").await.unwrap();

    let counts = lets_chat::db::chat::list_dm_unread_counts(&pool, "user-a").await.unwrap();
    assert!(counts.iter().all(|(r, _)| *r != room.id));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `docker compose run --rm dev cargo test --test db_read_receipts`
Expected: FAIL — `upsert_dm_read`, `get_dm_read_state`, `list_dm_unread_counts` not defined.

- [ ] **Step 3: Add read-state types and queries to `src/db/chat.rs`**

Append to `src/db/chat.rs`:

```rust
#[derive(Debug, Clone)]
pub struct DmReadState {
    pub user_id: String,
    pub room_id: i64,
    pub last_read_message_id: i64,
    pub updated_at: String,
}

/// Upsert the caller's last-read watermark for a DM. Monotonic: never decreases.
pub async fn upsert_dm_read(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
    message_id: i64,
) -> Result<String, sqlx::Error> {
    let updated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "INSERT INTO dm_read_state (user_id, room_id, last_read_message_id, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, room_id) DO UPDATE SET \
           last_read_message_id = MAX(excluded.last_read_message_id, dm_read_state.last_read_message_id), \
           updated_at = CASE \
             WHEN excluded.last_read_message_id > dm_read_state.last_read_message_id \
             THEN excluded.updated_at ELSE dm_read_state.updated_at END",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(message_id)
    .bind(&updated_at)
    .execute(pool)
    .await?;
    Ok(updated_at)
}

pub async fn get_dm_read_state(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<Option<DmReadState>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT user_id, room_id, last_read_message_id, updated_at \
         FROM dm_read_state WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| DmReadState {
        user_id: r.get("user_id"),
        room_id: r.get("room_id"),
        last_read_message_id: r.get("last_read_message_id"),
        updated_at: r.get("updated_at"),
    }))
}

/// For each DM the user is a member of, count peer messages newer than the user's watermark.
pub async fn list_dm_unread_counts(
    pool: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT r.id AS room_id, \
                COUNT(m.id) AS unread \
         FROM rooms r \
         JOIN room_members rm ON rm.room_id = r.id AND rm.user_id = ? \
         LEFT JOIN dm_read_state s ON s.room_id = r.id AND s.user_id = ? \
         LEFT JOIN messages m \
           ON m.room_id = r.id \
          AND m.user_id != ? \
          AND m.deleted_at IS NULL \
          AND m.id > COALESCE(s.last_read_message_id, 0) \
         WHERE r.room_type = 'dm' \
         GROUP BY r.id",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.get("room_id"), r.get::<i64, _>("unread"))).collect())
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `docker compose run --rm dev cargo test --test db_read_receipts`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/db/chat.rs tests/db_read_receipts.rs
git commit -m "feat(db): add dm read-state upsert and unread counts"
```

---

## Task 4: WebSocket event — `DmRead`

**Files:**
- Modify: `src/ws/events.rs`

- [ ] **Step 1: Add the variant**

In `src/ws/events.rs`, add inside `enum ChatEvent`:

```rust
    DmRead {
        room_id: i64,
        user_id: String,
        last_read_message_id: i64,
        read_at: String,
    },
```

- [ ] **Step 2: Build**

Run: `docker compose run --rm dev cargo build --all-targets`
Expected: clean build (serde derive covers the new variant; no exhaustive match bugs expected but the compiler will flag any).

- [ ] **Step 3: Commit**

```bash
git add src/ws/events.rs
git commit -m "feat(ws): add DmRead event"
```

---

## Task 5: Server fns — `mark_dm_read`, `get_dm_peer_read_state`, `list_dm_unread_counts_fn`

**Files:**
- Modify: `src/server_fns/dm.rs`

- [ ] **Step 1: Add `PeerReadState` DTO**

At the top of `src/server_fns/dm.rs` (next to `DmConversation`), add:

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerReadState {
    pub last_read_message_id: i64,
    pub read_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DmUnread {
    pub room_id: i64,
    pub count: i64,
}
```

- [ ] **Step 2: Add `mark_dm_read` server fn**

Append to `src/server_fns/dm.rs`:

```rust
#[server]
pub async fn mark_dm_read(room_id: i64, message_id: i64) -> Result<(), ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;

    let chat_pool = crate::db::get_chat_pool().await;
    let room = crate::db::chat::get_room(chat_pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Room not found"))?;
    if room.room_type != "dm" {
        return Err(ServerFnError::new("Not a DM"));
    }

    // Verify the message belongs to this room (prevents smuggling a stranger's id)
    let msg = crate::db::chat::get_message(chat_pool, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Message not found"))?;
    if msg.room_id != room_id {
        return Err(ServerFnError::new("Message not in this room"));
    }

    let is_member = crate::db::chat::is_room_member(chat_pool, room_id, &me.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !is_member {
        return Err(ServerFnError::new("Not a member of this DM"));
    }

    let read_at = crate::db::chat::upsert_dm_read(chat_pool, &me.id, room_id, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Symmetric-consent broadcast: only if both users have receipts enabled.
    let auth_pool = crate::db::get_auth_pool().await;
    let peer_id = {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM room_members WHERE room_id = ? AND user_id != ? LIMIT 1",
        )
        .bind(room_id)
        .bind(&me.id)
        .fetch_optional(chat_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        rows
    };

    if me.read_receipts_enabled {
        if let Some(peer_id) = peer_id {
            if let Ok(Some(peer)) = crate::db::auth::find_user_by_id(auth_pool, &peer_id).await {
                if peer.read_receipts_enabled {
                    let event = crate::ws::events::ChatEvent::DmRead {
                        room_id,
                        user_id: me.id.clone(),
                        last_read_message_id: message_id,
                        read_at,
                    };
                    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Add `get_dm_peer_read_state` server fn**

Append:

```rust
#[server]
pub async fn get_dm_peer_read_state(room_id: i64) -> Result<Option<PeerReadState>, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;
    if !me.read_receipts_enabled {
        return Ok(None);
    }

    let chat_pool = crate::db::get_chat_pool().await;
    let auth_pool = crate::db::get_auth_pool().await;

    let peer_id: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM room_members WHERE room_id = ? AND user_id != ? LIMIT 1",
    )
    .bind(room_id)
    .bind(&me.id)
    .fetch_optional(chat_pool)
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(peer_id) = peer_id else { return Ok(None) };

    let peer = crate::db::auth::find_user_by_id(auth_pool, &peer_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if peer.map(|p| !p.read_receipts_enabled).unwrap_or(true) {
        return Ok(None);
    }

    let state = crate::db::chat::get_dm_read_state(chat_pool, &peer_id, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(state.map(|s| PeerReadState {
        last_read_message_id: s.last_read_message_id,
        read_at: s.updated_at,
    }))
}
```

- [ ] **Step 4: Add `list_dm_unread_counts_fn` server fn**

Append:

```rust
#[server]
pub async fn list_dm_unread_counts_fn() -> Result<Vec<DmUnread>, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;
    let chat_pool = crate::db::get_chat_pool().await;
    let rows = crate::db::chat::list_dm_unread_counts(chat_pool, &me.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(room_id, count)| DmUnread { room_id, count })
        .collect())
}
```

- [ ] **Step 5: Build**

Run: `docker compose run --rm dev cargo build --all-targets`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add src/server_fns/dm.rs
git commit -m "feat(server_fns): add mark_dm_read, peer read state, and unread counts"
```

---

## Task 6: Server fn — `set_read_receipts_enabled`

**Files:**
- Modify: `src/server_fns/auth.rs`

- [ ] **Step 1: Add the server fn**

Append to `src/server_fns/auth.rs`:

```rust
#[server]
pub async fn set_read_receipts_enabled(enabled: bool) -> Result<(), ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_auth_pool().await;
    crate::db::auth::set_read_receipts_enabled(pool, &me.id, enabled)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
```

- [ ] **Step 2: Build**

Run: `docker compose run --rm dev cargo build --all-targets`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add src/server_fns/auth.rs
git commit -m "feat(server_fns): add set_read_receipts_enabled"
```

---

## Task 7: DM view — mark read on mount, visibility, and new messages

**Files:**
- Modify: `src/components/dm_view.rs`

- [ ] **Step 1: Import new server fns and add signals**

At the top of `src/components/dm_view.rs`, update the server fn import:

```rust
use crate::server_fns::dm::{get_dm_peer_read_state, get_or_create_dm, mark_dm_read, send_dm_message};
```

After the existing `let mut edit_error = ...;` block (around line 78), add:

```rust
let mut peer_last_read_id = use_signal(|| Option::<i64>::None);
let mut peer_read_at = use_signal(|| Option::<String>::None);
```

- [ ] **Step 2: Fetch peer read state on mount**

After the messages `use_server_future` block, add:

```rust
let peer_state = use_server_future(move || async move {
    get_dm_peer_read_state(room_id).await
})?;

use_effect(move || {
    if let Some(Ok(Some(s))) = peer_state() {
        peer_last_read_id.set(Some(s.last_read_message_id));
        peer_read_at.set(Some(s.read_at));
    }
});
```

- [ ] **Step 3: Mark-read helper — call when appropriate**

Add a `use_effect` that watches messages + visibility. Insert after the existing typing `use_effect` (around line 111):

```rust
let my_id_for_read = u.id.clone();
use_effect(move || {
    let list = match messages() { Some(Ok(l)) => l, _ => return };
    let latest_peer = list.iter().rev().find(|m| m.user_id != my_id_for_read);
    let Some(latest) = latest_peer else { return };
    let latest_id = latest.id;

    #[cfg(target_arch = "wasm32")]
    {
        let visible = web_sys::window()
            .and_then(|w| w.document())
            .map(|d| d.visibility_state() == web_sys::VisibilityState::Visible)
            .unwrap_or(true);
        if !visible { return; }
    }

    spawn(async move {
        let _ = mark_dm_read(room_id, latest_id).await;
    });
});
```

- [ ] **Step 4: Install `visibilitychange` listener**

Append one more `use_effect` that installs a listener once the component mounts. This bumps `messages_version` so the previous effect re-runs when tab becomes visible:

```rust
#[cfg(target_arch = "wasm32")]
{
    use_effect(move || {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let cb = Closure::<dyn FnMut()>::new(move || {
            let v = *messages_version.peek();
            messages_version.set(v + 1);
        });
        let _ = document.add_event_listener_with_callback(
            "visibilitychange",
            cb.as_ref().unchecked_ref(),
        );
        // leak the closure — lives for the component's lifetime; harmless given DM views are short-lived
        cb.forget();
    });
}
```

- [ ] **Step 5: Handle `DmRead` events in the existing WS effect**

In the existing `use_effect` that matches on `*ws.latest_event.read()`, add a new arm inside the first `match event { ... }` (the messages version one, around line 61):

```rust
ChatEvent::DmRead { room_id: event_room_id, last_read_message_id, read_at, .. }
    if *event_room_id == room_id =>
{
    peer_last_read_id.set(Some(*last_read_message_id));
    peer_read_at.set(Some(read_at.clone()));
}
```

- [ ] **Step 6: Render "Seen" label under the last-read own message**

Modify the `for msg in message_list.iter()` block: after the existing message rendering, conditionally render a "Seen" caption. Inside the `rsx!` that returns the message `div`, right after the `else { p { ... } }`, add:

```rust
{
    let last_read = peer_last_read_id();
    let show_seen = is_own && last_read == Some(msg_id);
    // Only show for the most recent own-authored message ≤ last_read.
    // Simple approach: render under every own msg with id == last_read.
    if show_seen {
        let read_at = peer_read_at().unwrap_or_default();
        // Extract HH:MM from "YYYY-MM-DD HH:MM:SS"
        let hhmm = read_at.split(' ').nth(1).map(|t| &t[..5.min(t.len())]).unwrap_or("").to_string();
        rsx! { div { class: "text-xs text-gray-400 mt-0.5", "Seen {hhmm}" } }
    } else {
        rsx! {}
    }
}
```

Note: because `peer_last_read_id` is the exact id the peer reached, and the previous own-authored messages are implicitly covered by monotonicity, showing the label on exact match is sufficient. If the peer read a message beyond the last own one, find the highest own id ≤ last_read:

Replace the `show_seen` lines above with:

```rust
let show_seen = is_own && last_read.map(|lr| {
    // this message is the highest own-authored id ≤ lr
    msg_id <= lr &&
    message_list.iter().rev()
        .find(|m| m.user_id == u.id && m.id <= lr)
        .map(|m| m.id == msg_id).unwrap_or(false)
}).unwrap_or(false);
```

- [ ] **Step 7: Build**

Run: `docker compose run --rm dev cargo build --all-targets`
Expected: clean build.

- [ ] **Step 8: Commit**

```bash
git add src/components/dm_view.rs
git commit -m "feat(dm): mark read on visibility and render Seen label"
```

---

## Task 8: Sidebar — unread badges

**Files:**
- Modify: `src/components/sidebar.rs`

- [ ] **Step 1: Import and fetch unread counts**

Update imports in `src/components/sidebar.rs`:

```rust
use crate::server_fns::dm::{list_dm_unread_counts_fn, list_my_dms};
```

After the existing `dms` future (around line 27), add:

```rust
let mut unread_version = use_signal(|| 0u32);
let unread = use_server_future(move || {
    let _v = unread_version();
    async move { list_dm_unread_counts_fn().await }
})?;
```

- [ ] **Step 2: Bump unread on relevant WS events and on route change**

In the existing WS `use_effect`, extend the `match`:

```rust
ChatEvent::NewMessage { is_dm: true, .. } => {
    let v = *dms_version.peek(); dms_version.set(v + 1);
    let v = *unread_version.peek(); unread_version.set(v + 1);
}
ChatEvent::DmRead { .. } => {
    let v = *unread_version.peek(); unread_version.set(v + 1);
}
```

- [ ] **Step 3: Build a lookup map and render badges**

Before the `rsx!` return (after `dm_list`), add:

```rust
let unread_map: std::collections::HashMap<i64, i64> = match unread() {
    Some(Ok(list)) => list.into_iter().map(|u| (u.room_id, u.count)).collect(),
    _ => std::collections::HashMap::new(),
};
```

Inside the `for dm in dm_list.iter()` block, change the `Link` children to include a badge:

```rust
for dm in dm_list.iter() {
    let count = unread_map.get(&dm.room_id).copied().unwrap_or(0);
    rsx! {
        Link {
            key: "{dm.room_id}",
            to: Route::Dm { user_id: dm.other_user_id.clone() },
            class: "flex items-center gap-2 px-3 py-1.5 text-sm rounded hover:bg-gray-100 text-gray-700",
            span { class: "text-gray-400", "@" }
            span { class: "flex-1", "{dm.other_user_name}" }
            if count > 0 {
                span {
                    class: "ml-auto inline-flex items-center justify-center min-w-[1.25rem] h-5 px-1.5 rounded-full bg-blue-600 text-white text-xs font-semibold",
                    "{count}"
                }
            }
        }
    }
}
```

(Wrap each iteration in `{ ... rsx! { ... } }` to match the surrounding Dioxus pattern if needed — match the existing block's shape.)

- [ ] **Step 4: Build**

Run: `docker compose run --rm dev cargo build --all-targets`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add src/components/sidebar.rs
git commit -m "feat(sidebar): show DM unread count badges"
```

---

## Task 9: Settings toggle UI

**Files:**
- Modify: `src/components/sidebar.rs` (add a popover or expandable settings under the user row)

- [ ] **Step 1: Add local signal and handler**

Near the top of `Sidebar` (after `let mut unread_version = ...;`), add:

```rust
let mut show_settings = use_signal(|| false);
let mut receipts_enabled = use_signal(|| u.read_receipts_enabled);
```

- [ ] **Step 2: Import the server fn**

```rust
use crate::server_fns::auth::{clear_session_cookie, logout, set_read_receipts_enabled};
```

- [ ] **Step 3: Render a gear button and popover**

In the bottom user-info div (around line 122), replace the existing `button {` logout-only block with a row that also includes a gear button, and add a popover shown when `show_settings()`:

```rust
// Gear button
button {
    class: "text-xs text-gray-500 hover:text-blue-600 flex-shrink-0",
    r#type: "button",
    onclick: move |_| {
        let cur = show_settings();
        show_settings.set(!cur);
    },
    "⚙"
}
// existing Logout button unchanged
```

Above the user-info div, add:

```rust
if show_settings() {
    div { class: "px-3 py-2 border-t border-gray-200 bg-gray-50 text-xs",
        label { class: "flex items-center gap-2 cursor-pointer",
            input {
                r#type: "checkbox",
                checked: receipts_enabled(),
                onchange: move |e| {
                    let new_val = e.value() == "true" || e.checked();
                    spawn(async move {
                        if set_read_receipts_enabled(new_val).await.is_ok() {
                            receipts_enabled.set(new_val);
                        }
                    });
                },
            }
            span { "Send and receive read receipts" }
        }
    }
}
```

(Replace `e.checked()` usage with whatever the current Dioxus 0.7 API exposes — check `login.rs` or `register.rs` for the pattern this project uses for checkbox input.)

- [ ] **Step 4: Build**

Run: `docker compose run --rm dev cargo build --all-targets`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add src/components/sidebar.rs
git commit -m "feat(sidebar): add read-receipts toggle in user menu"
```

---

## Task 10: Manual verification + final test run

**Files:** none

- [ ] **Step 1: Full check**

Run: `just check`
Expected: cargo server check, web check, clippy, fmt all pass.

- [ ] **Step 2: Full tests**

Run: `docker compose run --rm dev cargo test --all-targets`
Expected: all tests pass, including the new `db_read_receipts` suite.

- [ ] **Step 3: Manual browser verification**

Run: `just dev-web-local`, open two browsers as two users.

Verify:
1. User A DMs User B. User B opens DM → A sees "Seen HH:MM" under the last message.
2. While B's tab is hidden, A sends a message. B's sidebar shows unread count badge. B switches to tab → badge clears, A sees "Seen" update.
3. B disables read receipts in settings. A no longer sees "Seen" labels update for new reads. B still sees unread counts for themselves.
4. B re-enables → future reads broadcast again.

Document any UI issues found; fix in follow-up commits.

- [ ] **Step 4: Final commit if fixes were made**

```bash
git add -u
git commit -m "fix(read-receipts): <summary of fix>"
```

---

## Self-Review Checklist

- [x] Spec section "Data model" → Tasks 1, 2
- [x] Spec "Server functions" (mark_dm_read, set_read_receipts_enabled, list_dm_unread_counts, get_dm_peer_read_state) → Tasks 5, 6
- [x] Spec "WebSocket" DmRead variant → Task 4
- [x] Spec "Client behavior" (DM view mark-read, Seen label, DmRead handler) → Task 7
- [x] Spec "Sidebar unread badges" → Task 8
- [x] Spec "Settings toggle" → Task 9
- [x] Spec "Privacy semantics" (symmetric gating in broadcast + peer state fetch) → Tasks 5
- [x] Spec "Testing" (db_read_receipts integration tests) → Task 3
- [x] Spec "Build sequence" order preserved
