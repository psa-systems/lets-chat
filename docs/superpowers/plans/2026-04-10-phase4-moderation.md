# Phase 4: Moderation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add moderation tools — ban, mute, kick, suspend, message deletion — with an audit log, moderator-visible action buttons in the chat UI, and a Mod Log admin tab.

**Architecture:** A new `mod_actions` table in chat.db logs every moderation action. The messages table gets `deleted_at`/`deleted_by` columns for soft deletes. Auth DB ban/mute fields (already present in `users`) are updated by new DB functions. Server functions require `moderator` role minimum. The admin panel gains a "Mod Log" tab visible to both admins and moderators. The room view shows action buttons on messages/usernames for moderators+. Note: WebSocket real-time broadcast of mod events is deferred to Phase 6.

**Tech Stack:** Dioxus 0.7.3, sqlx, existing RBAC helpers (`require_role("moderator")`).

---

## File Map

| Action | Path | Responsibility |
|--------|------|---------------|
| Create | `migrations/chat/0002_moderation.sql` | Add `mod_actions` table, `deleted_at`/`deleted_by` to messages |
| Modify | `src/db/mod.rs` | Run `0002_moderation.sql` migration in `init_chat_pool` |
| Create | `src/models/mod_action.rs` | `ModAction` struct (serde) |
| Modify | `src/models/mod.rs` | Register `mod_action` module |
| Modify | `src/models/user.rs` | Add `is_banned`/`ban_reason` to `User` (public struct) |
| Create | `src/db/moderation.rs` | DB functions: ban, mute, kick, suspend, delete_message, list_mod_actions |
| Modify | `src/db/mod.rs` | Register `moderation` module |
| Modify | `src/db/chat.rs` | Update `list_messages` to filter soft-deleted, add `soft_delete_message` |
| Create | `src/server_fns/moderation.rs` | Server functions for mod actions |
| Modify | `src/server_fns/mod.rs` | Register `moderation` module |
| Modify | `src/components/room_view.rs` | Show mod action buttons for moderators+, show mute banner |
| Create | `src/components/admin/mod_log.rs` | Mod Log page |
| Modify | `src/components/admin/mod.rs` | Register `mod_log` module |
| Modify | `src/components/admin/layout.rs` | Add "Mod Log" tab, show reduced tabs for moderators |
| Modify | `src/components/admin/users.rs` | Add ban/mute/suspend buttons for moderators+ |
| Modify | `src/routes.rs` | Add `/admin/modlog` route |
| Modify | `src/components/sidebar.rs` | Show Admin link for moderators too (not just admins) |
| Create | `tests/db_moderation.rs` | Tests for moderation DB functions |

---

### Task 1: Chat DB Migration for Moderation

**Files:**
- Create: `migrations/chat/0002_moderation.sql`
- Modify: `src/db/mod.rs`

- [ ] **Step 1: Create the migration file**

Create `migrations/chat/0002_moderation.sql`:

```sql
-- Soft-delete columns for messages
ALTER TABLE messages ADD COLUMN deleted_at TEXT;
ALTER TABLE messages ADD COLUMN deleted_by TEXT;

-- Moderation audit log
CREATE TABLE IF NOT EXISTS mod_actions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    action      TEXT NOT NULL,
    target_user TEXT NOT NULL,
    actor_user  TEXT NOT NULL,
    reason      TEXT,
    room_id     INTEGER,
    metadata    TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_mod_actions_created_at ON mod_actions(created_at);
CREATE INDEX IF NOT EXISTS idx_mod_actions_target ON mod_actions(target_user);
```

- [ ] **Step 2: Register migration in init_chat_pool**

In `src/db/mod.rs`, inside `init_chat_pool()`, after running `0001_create_tables.sql`, add:

```rust
let migration_002 = include_str!("../../migrations/chat/0002_moderation.sql");
sqlx::raw_sql(migration_002)
    .execute(&pool)
    .await
    .expect("Failed to run chat DB migration 002");
```

- [ ] **Step 3: Verify compilation**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
```

- [ ] **Step 4: Commit**

```bash
git add migrations/chat/0002_moderation.sql src/db/mod.rs
git commit -m "feat: add moderation migration (mod_actions table, soft-delete columns)"
```

---

### Task 2: ModAction Model and DB Functions

**Files:**
- Create: `src/models/mod_action.rs`
- Modify: `src/models/mod.rs`
- Modify: `src/models/user.rs`
- Create: `src/db/moderation.rs`
- Modify: `src/db/mod.rs`
- Modify: `src/db/chat.rs`

- [ ] **Step 1: Create the ModAction model**

Create `src/models/mod_action.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModAction {
    pub id: i64,
    pub action: String,
    pub target_user: String,
    pub actor_user: String,
    pub reason: Option<String>,
    pub room_id: Option<i64>,
    pub metadata: Option<String>,
    pub created_at: String,
}
```

- [ ] **Step 2: Register in models/mod.rs**

Add to `src/models/mod.rs`:

```rust
pub mod mod_action;
pub use mod_action::ModAction;
```

- [ ] **Step 3: Add ban fields to public User struct**

In `src/models/user.rs`, add to the `User` struct (after `is_muted`/`muted_until`):

```rust
pub is_banned: bool,
pub ban_reason: Option<String>,
pub banned_until: Option<String>,
```

These are needed so the admin users page can display ban status and moderators can see who is banned.

- [ ] **Step 4: Update all User construction sites**

Every place that constructs a `User` from a `UserRecord` needs the new fields. There are two locations:

In `src/server_fns/auth.rs`, in `get_current_user()` where `User { ... }` is constructed, add:

```rust
is_banned: record.is_banned,
ban_reason: record.ban_reason.clone(),
banned_until: record.banned_until.clone(),
```

In `src/server_fns/admin.rs`, in `list_users()` where `User { ... }` is constructed, add:

```rust
is_banned: r.is_banned,
ban_reason: r.ban_reason,
banned_until: r.banned_until,
```

- [ ] **Step 5: Create src/db/moderation.rs with moderation DB functions**

Create `src/db/moderation.rs`:

```rust
use sqlx::{Row, SqlitePool};

use crate::models::mod_action::ModAction;

/// Record a moderation action in the audit log (chat.db).
pub async fn log_mod_action(
    pool: &SqlitePool,
    action: &str,
    target_user: &str,
    actor_user: &str,
    reason: Option<&str>,
    room_id: Option<i64>,
    metadata: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO mod_actions (action, target_user, actor_user, reason, room_id, metadata) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(action)
    .bind(target_user)
    .bind(actor_user)
    .bind(reason)
    .bind(room_id)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// List mod actions ordered by most recent first.
pub async fn list_mod_actions(pool: &SqlitePool) -> Result<Vec<ModAction>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, action, target_user, actor_user, reason, room_id, metadata, created_at \
         FROM mod_actions ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ModAction {
            id: r.get("id"),
            action: r.get("action"),
            target_user: r.get("target_user"),
            actor_user: r.get("actor_user"),
            reason: r.get("reason"),
            room_id: r.get("room_id"),
            metadata: r.get("metadata"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Soft-delete a message by setting deleted_at and deleted_by.
pub async fn soft_delete_message(
    pool: &SqlitePool,
    message_id: i64,
    deleted_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = ? WHERE id = ?",
    )
    .bind(deleted_by)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 6: Add ban/mute/suspend DB functions to src/db/auth.rs**

Add these functions to `src/db/auth.rs`:

```rust
pub async fn ban_user(
    pool: &SqlitePool,
    user_id: &str,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_banned = 1, ban_reason = ?, banned_until = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(reason)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unban_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_banned = 0, ban_reason = NULL, banned_until = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn suspend_user(
    pool: &SqlitePool,
    user_id: &str,
    until: &str,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_banned = 1, ban_reason = ?, banned_until = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(reason)
    .bind(until)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mute_user(
    pool: &SqlitePool,
    user_id: &str,
    until: Option<&str>,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_muted = 1, muted_until = ?, mute_reason = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(until)
    .bind(reason)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unmute_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_muted = 0, muted_until = NULL, mute_reason = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] **Step 7: Register moderation module in db/mod.rs**

Add to `src/db/mod.rs` (inside the `#[cfg(not(target_arch = "wasm32"))]` block):

```rust
#[cfg(not(target_arch = "wasm32"))]
pub mod moderation;
```

- [ ] **Step 8: Update list_messages to filter soft-deleted messages**

In `src/db/chat.rs`, change the `list_messages` query from:

```rust
"SELECT id, room_id, user_id, body, created_at \
 FROM messages WHERE room_id = ? ORDER BY id ASC",
```

to:

```rust
"SELECT id, room_id, user_id, body, created_at \
 FROM messages WHERE room_id = ? AND deleted_at IS NULL ORDER BY id ASC",
```

- [ ] **Step 9: Verify compilation**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
```

- [ ] **Step 10: Commit**

```bash
git add src/models/mod_action.rs src/models/mod.rs src/models/user.rs \
        src/db/moderation.rs src/db/mod.rs src/db/auth.rs src/db/chat.rs \
        src/server_fns/auth.rs src/server_fns/admin.rs
git commit -m "feat: add moderation models, DB functions, and soft-delete filtering"
```

---

### Task 3: Moderation Server Functions

**Files:**
- Create: `src/server_fns/moderation.rs`
- Modify: `src/server_fns/mod.rs`

- [ ] **Step 1: Create src/server_fns/moderation.rs**

Create `src/server_fns/moderation.rs`:

```rust
use dioxus::prelude::*;

use crate::models::ModAction;

#[server]
pub async fn ban_user(user_id: String, reason: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let reason_opt = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim().to_string())
    };

    let auth_pool = crate::db::get_auth_pool().await;

    // Verify target exists
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    // Moderators cannot ban admins or other moderators
    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    crate::db::auth::ban_user(auth_pool, &user_id, reason_opt.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Invalidate all sessions for banned user
    crate::db::auth::delete_user_sessions(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Log the action
    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "ban",
        &user_id,
        &actor.id,
        reason_opt.as_deref(),
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn unban_user(user_id: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    crate::db::auth::unban_user(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "unban",
        &user_id,
        &actor.id,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn suspend_user(
    user_id: String,
    until: String,
    reason: String,
) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let reason_opt = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim().to_string())
    };

    let auth_pool = crate::db::get_auth_pool().await;

    // Verify target and check hierarchy
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    crate::db::auth::suspend_user(auth_pool, &user_id, &until, reason_opt.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Invalidate sessions
    crate::db::auth::delete_user_sessions(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    let metadata = serde_json::json!({ "until": until }).to_string();
    crate::db::moderation::log_mod_action(
        chat_pool,
        "suspend",
        &user_id,
        &actor.id,
        reason_opt.as_deref(),
        None,
        Some(&metadata),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn mute_user(
    user_id: String,
    until: String,
    reason: String,
) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let reason_opt = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim().to_string())
    };
    let until_opt = if until.trim().is_empty() {
        None
    } else {
        Some(until.trim().to_string())
    };

    let auth_pool = crate::db::get_auth_pool().await;

    // Verify target and check hierarchy
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    crate::db::auth::mute_user(auth_pool, &user_id, until_opt.as_deref(), reason_opt.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    let metadata = until_opt
        .as_ref()
        .map(|u| serde_json::json!({ "until": u }).to_string());
    crate::db::moderation::log_mod_action(
        chat_pool,
        "mute",
        &user_id,
        &actor.id,
        reason_opt.as_deref(),
        None,
        metadata.as_deref(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn unmute_user(user_id: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    crate::db::auth::unmute_user(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "unmute",
        &user_id,
        &actor.id,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn delete_message(message_id: i64, reason: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let reason_opt = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim().to_string())
    };

    let chat_pool = crate::db::get_chat_pool().await;

    // Get the message to find the target user and room
    let row = sqlx::query(
        "SELECT user_id, room_id FROM messages WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(message_id)
    .fetch_optional(chat_pool)
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?
    .ok_or_else(|| ServerFnError::new("Message not found"))?;

    let target_user: String = sqlx::Row::get(&row, "user_id");
    let room_id: i64 = sqlx::Row::get(&row, "room_id");

    crate::db::moderation::soft_delete_message(chat_pool, message_id, &actor.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let metadata = serde_json::json!({ "message_id": message_id }).to_string();
    crate::db::moderation::log_mod_action(
        chat_pool,
        "delete_message",
        &target_user,
        &actor.id,
        reason_opt.as_deref(),
        Some(room_id),
        Some(&metadata),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn kick_user(user_id: String, room_id: i64, reason: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let reason_opt = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim().to_string())
    };

    let auth_pool = crate::db::get_auth_pool().await;

    // Verify target and check hierarchy
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    // Log the kick (kick is a notification-only action until WebSocket phase)
    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "kick",
        &user_id,
        &actor.id,
        reason_opt.as_deref(),
        Some(room_id),
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

#[server]
pub async fn list_mod_actions() -> Result<Vec<ModAction>, ServerFnError> {
    crate::server_fns::helpers::require_role("moderator").await?;

    let pool = crate::db::get_chat_pool().await;
    crate::db::moderation::list_mod_actions(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
```

- [ ] **Step 2: Register moderation module in server_fns/mod.rs**

Update `src/server_fns/mod.rs`:

```rust
pub mod admin;
pub mod auth;
pub mod chat;
#[cfg(feature = "server")]
pub mod helpers;
pub mod moderation;
```

- [ ] **Step 3: Verify compilation**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
```

- [ ] **Step 4: Commit**

```bash
git add src/server_fns/moderation.rs src/server_fns/mod.rs
git commit -m "feat: add moderation server functions (ban, mute, suspend, kick, delete_message)"
```

---

### Task 4: Moderation Tests

**Files:**
- Create: `tests/db_moderation.rs`

- [ ] **Step 1: Create moderation DB tests**

Create `tests/db_moderation.rs`:

```rust
use sqlx::SqlitePool;

async fn setup_pools() -> (SqlitePool, SqlitePool) {
    let auth_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("auth pool");
    let auth_migration = include_str!("../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(auth_migration)
        .execute(&auth_pool)
        .await
        .expect("auth migration");

    let chat_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    let chat_m1 = include_str!("../migrations/chat/0001_create_tables.sql");
    sqlx::raw_sql(chat_m1)
        .execute(&chat_pool)
        .await
        .expect("chat migration 1");
    let chat_m2 = include_str!("../migrations/chat/0002_moderation.sql");
    sqlx::raw_sql(chat_m2)
        .execute(&chat_pool)
        .await
        .expect("chat migration 2");

    (auth_pool, chat_pool)
}

#[tokio::test]
async fn test_ban_user() {
    let (auth_pool, _) = setup_pools().await;
    let id = lets_chat::db::auth::create_user(&auth_pool, "alice", "hash")
        .await
        .unwrap();

    lets_chat::db::auth::ban_user(&auth_pool, &id, Some("spam"))
        .await
        .unwrap();

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_banned);
    assert_eq!(user.ban_reason.as_deref(), Some("spam"));
    assert!(user.banned_until.is_none());
}

#[tokio::test]
async fn test_unban_user() {
    let (auth_pool, _) = setup_pools().await;
    let id = lets_chat::db::auth::create_user(&auth_pool, "alice", "hash")
        .await
        .unwrap();

    lets_chat::db::auth::ban_user(&auth_pool, &id, Some("spam"))
        .await
        .unwrap();
    lets_chat::db::auth::unban_user(&auth_pool, &id)
        .await
        .unwrap();

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(!user.is_banned);
    assert!(user.ban_reason.is_none());
}

#[tokio::test]
async fn test_suspend_user() {
    let (auth_pool, _) = setup_pools().await;
    let id = lets_chat::db::auth::create_user(&auth_pool, "alice", "hash")
        .await
        .unwrap();

    lets_chat::db::auth::suspend_user(&auth_pool, &id, "2099-12-31 23:59:59", Some("timeout"))
        .await
        .unwrap();

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_banned);
    assert_eq!(user.ban_reason.as_deref(), Some("timeout"));
    assert_eq!(user.banned_until.as_deref(), Some("2099-12-31 23:59:59"));
}

#[tokio::test]
async fn test_mute_and_unmute_user() {
    let (auth_pool, _) = setup_pools().await;
    let id = lets_chat::db::auth::create_user(&auth_pool, "alice", "hash")
        .await
        .unwrap();

    lets_chat::db::auth::mute_user(&auth_pool, &id, Some("2099-12-31 23:59:59"), Some("spam"))
        .await
        .unwrap();

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_muted);
    assert_eq!(user.mute_reason.as_deref(), Some("spam"));

    lets_chat::db::auth::unmute_user(&auth_pool, &id)
        .await
        .unwrap();

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(!user.is_muted);
    assert!(user.mute_reason.is_none());
}

#[tokio::test]
async fn test_log_and_list_mod_actions() {
    let (_, chat_pool) = setup_pools().await;

    lets_chat::db::moderation::log_mod_action(
        &chat_pool,
        "ban",
        "user-1",
        "admin-1",
        Some("spam"),
        None,
        None,
    )
    .await
    .unwrap();

    lets_chat::db::moderation::log_mod_action(
        &chat_pool,
        "mute",
        "user-2",
        "admin-1",
        Some("off-topic"),
        Some(1),
        None,
    )
    .await
    .unwrap();

    let actions = lets_chat::db::moderation::list_mod_actions(&chat_pool)
        .await
        .unwrap();
    assert_eq!(actions.len(), 2);
    // Most recent first
    assert_eq!(actions[0].action, "mute");
    assert_eq!(actions[1].action, "ban");
}

#[tokio::test]
async fn test_soft_delete_message() {
    let (_, chat_pool) = setup_pools().await;

    // Insert a message into room 1 (seeded by migration)
    lets_chat::db::chat::insert_message(&chat_pool, 1, "user-1", "hello")
        .await
        .unwrap();

    let msgs = lets_chat::db::chat::list_messages(&chat_pool, 1)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);

    lets_chat::db::moderation::soft_delete_message(&chat_pool, msgs[0].id, "mod-1")
        .await
        .unwrap();

    // Message should no longer appear in list_messages
    let msgs_after = lets_chat::db::chat::list_messages(&chat_pool, 1)
        .await
        .unwrap();
    assert_eq!(msgs_after.len(), 0);
}
```

- [ ] **Step 2: Run all tests**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test
```

Expected: all existing tests pass + 7 new moderation tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/db_moderation.rs
git commit -m "test: add moderation DB tests (ban, mute, suspend, soft-delete, audit log)"
```

---

### Task 5: Mod Log Admin Page and Route

**Files:**
- Create: `src/components/admin/mod_log.rs`
- Modify: `src/components/admin/mod.rs`
- Modify: `src/components/admin/layout.rs`
- Modify: `src/routes.rs`
- Modify: `src/components/sidebar.rs`

- [ ] **Step 1: Create the Mod Log page component**

Create `src/components/admin/mod_log.rs`:

```rust
use dioxus::prelude::*;

use crate::server_fns::moderation::list_mod_actions;

#[component]
pub fn AdminModLogPage() -> Element {
    let actions_future = use_server_future(list_mod_actions)?;

    let read_guard = actions_future.read();
    let actions = match &*read_guard {
        Some(Ok(a)) => a.clone(),
        Some(Err(e)) => {
            let err = e.to_string();
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading mod log: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading..." }
            };
        }
    };

    rsx! {
        div { class: "max-w-4xl mx-auto space-y-4",
            h2 { class: "text-lg font-semibold text-gray-800", "Moderation Log" }

            if actions.is_empty() {
                div { class: "text-gray-500 text-sm", "No moderation actions recorded yet." }
            } else {
                div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                    table { class: "w-full text-sm",
                        thead {
                            tr { class: "bg-gray-50 text-left",
                                th { class: "px-4 py-3 font-medium text-gray-500", "Time" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "Action" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "Target" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "By" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "Reason" }
                            }
                        }
                        tbody {
                            for action in actions.iter() {
                                {
                                    let action_class = match action.action.as_str() {
                                        "ban" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-red-100 text-red-800",
                                        "unban" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800",
                                        "suspend" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-orange-100 text-orange-800",
                                        "mute" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-yellow-100 text-yellow-800",
                                        "unmute" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800",
                                        "kick" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-purple-100 text-purple-800",
                                        "delete_message" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-gray-100 text-gray-800",
                                        _ => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-gray-100 text-gray-800",
                                    };
                                    let reason = action.reason.clone().unwrap_or_default();
                                    rsx! {
                                        tr { key: "{action.id}", class: "border-t border-gray-100",
                                            td { class: "px-4 py-3 text-gray-500 whitespace-nowrap", "{action.created_at}" }
                                            td { class: "px-4 py-3",
                                                span { class: action_class, "{action.action}" }
                                            }
                                            td { class: "px-4 py-3 font-medium text-gray-800", "{action.target_user}" }
                                            td { class: "px-4 py-3 text-gray-600", "{action.actor_user}" }
                                            td { class: "px-4 py-3 text-gray-600", "{reason}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Register mod_log in components/admin/mod.rs**

Add to `src/components/admin/mod.rs`:

```rust
pub mod mod_log;
```

- [ ] **Step 3: Add AdminModLog route**

In `src/routes.rs`, add the import:

```rust
use crate::components::admin::mod_log::AdminModLogPage;
```

Add the route variant inside the `Route` enum (after `AdminRooms`):

```rust
#[route("/admin/modlog")]
AdminModLog {},
```

Add the route component:

```rust
#[component]
fn AdminModLog() -> Element {
    rsx! { AdminModLogPage {} }
}
```

- [ ] **Step 4: Update admin layout to show Mod Log tab and support moderator access**

Replace the entire `src/components/admin/layout.rs` with:

```rust
use dioxus::prelude::*;

use crate::models::User;
use crate::routes::Route;

#[component]
pub fn AdminLayout() -> Element {
    let user: Signal<User> = use_context::<Signal<User>>();
    let u = user();

    let mut tabs: Vec<(&str, Route)> = vec![];

    if u.role == "admin" {
        tabs.push(("Settings", Route::AdminSettings {}));
    }

    // Both admins and moderators see Users and Mod Log
    tabs.push(("Users", Route::AdminUsers {}));

    if u.role == "admin" {
        tabs.push(("Invite Codes", Route::AdminInvites {}));
        tabs.push(("Rooms", Route::AdminRooms {}));
    }

    tabs.push(("Mod Log", Route::AdminModLog {}));

    let current_route = use_route::<Route>();

    rsx! {
        div { class: "flex-1 flex flex-col overflow-hidden",
            // Tab bar
            div { class: "flex border-b border-gray-200 bg-white px-4",
                for (label, route) in tabs {
                    {
                        let is_active = current_route == route;
                        let class = if is_active {
                            "px-4 py-2.5 text-sm font-medium border-b-2 -mb-px border-blue-500 text-blue-600"
                        } else {
                            "px-4 py-2.5 text-sm font-medium border-b-2 -mb-px border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                        };
                        rsx! {
                            Link {
                                to: route,
                                class: class,
                                "{label}"
                            }
                        }
                    }
                }
            }
            // Content area
            div { class: "flex-1 overflow-y-auto p-6 bg-gray-50",
                Outlet::<Route> {}
            }
        }
    }
}
```

- [ ] **Step 5: Update sidebar to show Admin link for moderators too**

In `src/components/sidebar.rs`, change the admin link condition from:

```rust
if u.role == "admin" {
```

to:

```rust
if u.role == "admin" || u.role == "moderator" {
```

Also change the link destination for moderators. Replace the entire admin link block with:

```rust
if u.role == "admin" || u.role == "moderator" {
    div { class: "px-2 mt-2",
        Link {
            to: if u.role == "admin" { Route::AdminSettings {} } else { Route::AdminUsers {} },
            class: "flex items-center gap-2 px-3 py-1.5 text-sm rounded hover:bg-red-50 text-red-600 font-medium",
            span { if u.role == "admin" { "Admin" } else { "Moderate" } }
        }
    }
}
```

- [ ] **Step 6: Verify compilation**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
```

- [ ] **Step 7: Commit**

```bash
git add src/components/admin/mod_log.rs src/components/admin/mod.rs \
        src/components/admin/layout.rs src/routes.rs src/components/sidebar.rs
git commit -m "feat: add Mod Log page, moderator access to admin panel"
```

---

### Task 6: Moderation UI on Admin Users Page

**Files:**
- Modify: `src/components/admin/users.rs`

- [ ] **Step 1: Add moderation action buttons to users table**

Replace the entire `src/components/admin/users.rs` with:

```rust
use dioxus::prelude::*;

use crate::models::User;
use crate::server_fns::admin::{change_user_role, delete_user, list_users};
use crate::server_fns::moderation::{
    ban_user, mute_user, suspend_user, unban_user, unmute_user,
};

#[component]
pub fn AdminUsersPage() -> Element {
    let current_user: Signal<User> = use_context::<Signal<User>>();
    let mut users_future = use_server_future(list_users)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut confirm_delete = use_signal(|| Option::<(String, String)>::None);
    let mut mod_modal = use_signal(|| Option::<ModModalState>::None);

    let read_guard = users_future.read();
    let users = match &*read_guard {
        Some(Ok(u)) => u.clone(),
        Some(Err(e)) => {
            let err = e.to_string();
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading users: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading users..." }
            };
        }
    };

    let cu = current_user();
    let is_admin = cu.role == "admin";

    rsx! {
        div { class: "max-w-5xl mx-auto space-y-4",
            h2 { class: "text-lg font-semibold text-gray-800", "Users" }

            if let Some((is_ok, msg)) = feedback() {
                div {
                    class: if is_ok { "p-3 rounded-lg bg-green-50 text-green-700 text-sm" } else { "p-3 rounded-lg bg-red-50 text-red-700 text-sm" },
                    "{msg}"
                }
            }

            // Delete confirmation modal
            if let Some((user_id, username)) = confirm_delete() {
                div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
                    div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                        h3 { class: "text-lg font-semibold text-gray-800 mb-2", "Delete User" }
                        p { class: "text-sm text-gray-600 mb-4",
                            "Are you sure you want to delete user "
                            span { class: "font-medium", "{username}" }
                            "? This cannot be undone."
                        }
                        div { class: "flex justify-end gap-2",
                            button {
                                class: "px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200",
                                onclick: move |_| confirm_delete.set(None),
                                "Cancel"
                            }
                            button {
                                class: "px-3 py-1.5 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700",
                                onclick: move |_| {
                                    let uid = user_id.clone();
                                    spawn(async move {
                                        confirm_delete.set(None);
                                        match delete_user(uid).await {
                                            Ok(()) => {
                                                feedback.set(Some((true, "User deleted.".to_string())));
                                                users_future.restart();
                                            }
                                            Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                        }
                                    });
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }

            // Mod action modal
            if let Some(state) = mod_modal() {
                { render_mod_modal(state, mod_modal, feedback, users_future) }
            }

            div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                table { class: "w-full text-sm",
                    thead {
                        tr { class: "bg-gray-50 text-left",
                            th { class: "px-4 py-3 font-medium text-gray-500", "Username" }
                            if is_admin {
                                th { class: "px-4 py-3 font-medium text-gray-500", "Role" }
                            }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Status" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Created" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Actions" }
                        }
                    }
                    tbody {
                        for user in users.iter() {
                            {
                                let user_id = user.id.clone();
                                let username = user.username.clone();
                                let role = user.role.clone();
                                let user_is_banned = user.is_banned;
                                let user_is_muted = user.is_muted;
                                let created = user.created_at.clone();

                                let status_text = if user_is_banned {
                                    "banned"
                                } else if user_is_muted {
                                    "muted"
                                } else {
                                    "active"
                                };
                                let status_class = if user_is_banned {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-red-100 text-red-800"
                                } else if user_is_muted {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-yellow-100 text-yellow-800"
                                } else {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800"
                                };

                                let uid_for_role = user_id.clone();
                                let uid_for_delete = user_id.clone();
                                let uname_for_delete = username.clone();
                                let uid_for_ban = user_id.clone();
                                let uname_for_ban = username.clone();
                                let uid_for_mute = user_id.clone();
                                let uname_for_mute = username.clone();
                                let uid_for_unban = user_id.clone();
                                let uid_for_unmute = user_id.clone();

                                rsx! {
                                    tr { key: "{user_id}", class: "border-t border-gray-100",
                                        td { class: "px-4 py-3 font-medium text-gray-800",
                                            "{username}"
                                            span { class: "ml-1 text-xs text-gray-400", "({role})" }
                                        }
                                        if is_admin {
                                            td { class: "px-4 py-3",
                                                select {
                                                    class: "border border-gray-300 rounded px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                                    value: "{role}",
                                                    oninput: move |e| {
                                                        let new_role = e.value();
                                                        let uid = uid_for_role.clone();
                                                        spawn(async move {
                                                            match change_user_role(uid, new_role).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "Role updated.".to_string())));
                                                                    users_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    option { value: "user", selected: role == "user", "user" }
                                                    option { value: "moderator", selected: role == "moderator", "moderator" }
                                                    option { value: "admin", selected: role == "admin", "admin" }
                                                }
                                            }
                                        }
                                        td { class: "px-4 py-3",
                                            span { class: status_class, "{status_text}" }
                                        }
                                        td { class: "px-4 py-3 text-gray-500", "{created}" }
                                        td { class: "px-4 py-3 space-x-2",
                                            if user_is_banned {
                                                button {
                                                    class: "text-xs text-green-600 hover:text-green-800 font-medium",
                                                    onclick: move |_| {
                                                        let uid = uid_for_unban.clone();
                                                        spawn(async move {
                                                            match unban_user(uid).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "User unbanned.".to_string())));
                                                                    users_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    "Unban"
                                                }
                                            } else {
                                                button {
                                                    class: "text-xs text-red-600 hover:text-red-800 font-medium",
                                                    onclick: move |_| {
                                                        mod_modal.set(Some(ModModalState {
                                                            action: "ban".to_string(),
                                                            user_id: uid_for_ban.clone(),
                                                            username: uname_for_ban.clone(),
                                                        }));
                                                    },
                                                    "Ban"
                                                }
                                            }
                                            if user_is_muted {
                                                button {
                                                    class: "text-xs text-green-600 hover:text-green-800 font-medium",
                                                    onclick: move |_| {
                                                        let uid = uid_for_unmute.clone();
                                                        spawn(async move {
                                                            match unmute_user(uid).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "User unmuted.".to_string())));
                                                                    users_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    "Unmute"
                                                }
                                            } else {
                                                button {
                                                    class: "text-xs text-yellow-600 hover:text-yellow-800 font-medium",
                                                    onclick: move |_| {
                                                        mod_modal.set(Some(ModModalState {
                                                            action: "mute".to_string(),
                                                            user_id: uid_for_mute.clone(),
                                                            username: uname_for_mute.clone(),
                                                        }));
                                                    },
                                                    "Mute"
                                                }
                                            }
                                            if is_admin {
                                                button {
                                                    class: "text-xs text-red-600 hover:text-red-800 font-medium",
                                                    onclick: move |_| {
                                                        confirm_delete.set(Some((uid_for_delete.clone(), uname_for_delete.clone())));
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ModModalState {
    action: String,
    user_id: String,
    username: String,
}

fn render_mod_modal(
    state: ModModalState,
    mut mod_modal: Signal<Option<ModModalState>>,
    mut feedback: Signal<Option<(bool, String)>>,
    mut users_future: Resource<Result<Vec<User>, dioxus::prelude::ServerFnError>>,
) -> Element {
    let mut reason = use_signal(String::new);
    let mut duration = use_signal(|| "1h".to_string());

    let title = match state.action.as_str() {
        "ban" => format!("Ban {}", state.username),
        "mute" => format!("Mute {}", state.username),
        _ => "Mod Action".to_string(),
    };

    let show_duration = state.action == "mute";

    rsx! {
        div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
            div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                h3 { class: "text-lg font-semibold text-gray-800 mb-4", "{title}" }
                div { class: "space-y-3",
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Reason" }
                        input {
                            class: "w-full px-3 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            placeholder: "Reason for action...",
                            value: "{reason}",
                            oninput: move |e| reason.set(e.value()),
                        }
                    }
                    if show_duration {
                        div {
                            label { class: "block text-sm font-medium text-gray-700 mb-1", "Duration" }
                            select {
                                class: "w-full px-3 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                value: "{duration}",
                                oninput: move |e| duration.set(e.value()),
                                option { value: "1h", "1 hour" }
                                option { value: "24h", "24 hours" }
                                option { value: "7d", "7 days" }
                                option { value: "30d", "30 days" }
                                option { value: "permanent", "Permanent" }
                            }
                        }
                    }
                }
                div { class: "flex justify-end gap-2 mt-4",
                    button {
                        class: "px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200",
                        onclick: move |_| mod_modal.set(None),
                        "Cancel"
                    }
                    button {
                        class: "px-3 py-1.5 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700",
                        onclick: move |_| {
                            let action = state.action.clone();
                            let uid = state.user_id.clone();
                            let r = reason();
                            let d = duration();
                            spawn(async move {
                                mod_modal.set(None);
                                let result = match action.as_str() {
                                    "ban" => ban_user(uid, r).await,
                                    "mute" => {
                                        let until = duration_to_datetime(&d);
                                        mute_user(uid, until, r).await
                                    }
                                    _ => Ok(()),
                                };
                                match result {
                                    Ok(()) => {
                                        feedback.set(Some((true, format!("{} applied.", action))));
                                        users_future.restart();
                                    }
                                    Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                }
                            });
                        },
                        "Confirm"
                    }
                }
            }
        }
    }
}

fn duration_to_datetime(d: &str) -> String {
    // Calculate a future datetime string from a duration shorthand
    // This runs client-side so we use a simple approach
    let hours = match d {
        "1h" => 1,
        "24h" => 24,
        "7d" => 24 * 7,
        "30d" => 24 * 30,
        "permanent" => return String::new(),
        _ => 1,
    };
    // We'll let the server interpret an empty string as permanent
    // For timed mutes, we send the offset and the server computes the datetime
    format!("{}", hours)
}
```

Note: The `duration_to_datetime` function returns a number of hours as a string. The mute server function will need to be updated to handle this — see step 2.

- [ ] **Step 2: Update the mute_user server function to accept hours**

In `src/server_fns/moderation.rs`, update the `mute_user` function. Replace the `until_opt` logic:

```rust
let until_opt = if until.trim().is_empty() {
    None
} else {
    // Parse as hours offset
    match until.trim().parse::<i64>() {
        Ok(hours) => {
            let dt = chrono::Utc::now() + chrono::Duration::hours(hours);
            Some(dt.format("%Y-%m-%d %H:%M:%S").to_string())
        }
        Err(_) => {
            // Treat as direct datetime string
            Some(until.trim().to_string())
        }
    }
};
```

Do the same for `suspend_user` — update the `until` parameter handling. Replace:

```rust
crate::db::auth::suspend_user(auth_pool, &user_id, &until, reason_opt.as_deref())
```

with:

```rust
let until_dt = match until.trim().parse::<i64>() {
    Ok(hours) => {
        let dt = chrono::Utc::now() + chrono::Duration::hours(hours);
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    }
    Err(_) => until.trim().to_string(),
};
crate::db::auth::suspend_user(auth_pool, &user_id, &until_dt, reason_opt.as_deref())
```

And update the metadata to use `until_dt`:

```rust
let metadata = serde_json::json!({ "until": until_dt }).to_string();
```

- [ ] **Step 3: Update admin list_users to require moderator (not admin)**

In `src/server_fns/admin.rs`, change `list_users` from:

```rust
crate::server_fns::helpers::require_role("admin").await?;
```

to:

```rust
crate::server_fns::helpers::require_role("moderator").await?;
```

This allows moderators to see the user list (for mod actions), matching the spec.

- [ ] **Step 4: Verify compilation**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
```

- [ ] **Step 5: Commit**

```bash
git add src/components/admin/users.rs src/server_fns/moderation.rs src/server_fns/admin.rs
git commit -m "feat: add moderation actions UI on admin users page"
```

---

### Task 7: Moderation UI on Room View (Message Actions)

**Files:**
- Modify: `src/components/room_view.rs`

- [ ] **Step 1: Add mod action buttons to messages and mute banner**

Replace the entire `src/components/room_view.rs` with:

```rust
use dioxus::prelude::*;

use crate::models::User;
use crate::server_fns::chat::{get_room, list_messages, send_message};
use crate::server_fns::moderation::delete_message;

#[component]
pub fn RoomViewPage(room_id: String) -> Element {
    let parsed_id: i64 = match room_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return rsx! {
                div { class: "flex-1 flex items-center justify-center text-red-500",
                    "Invalid room id"
                }
            };
        }
    };

    let user: Signal<User> = use_context::<Signal<User>>();
    let u = user();
    let is_mod = u.role == "admin" || u.role == "moderator";

    let room = use_server_future(move || async move { get_room(parsed_id).await })?;

    let mut messages_version = use_signal(|| 0u32);
    let messages = use_server_future(move || {
        let _v = messages_version();
        async move { list_messages(parsed_id).await }
    })?;

    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut delete_reason = use_signal(String::new);
    let mut confirm_delete_msg = use_signal(|| Option::<i64>::None);

    let room_name = match room() {
        Some(Ok(Some(r))) => r.name,
        _ => format!("room {}", parsed_id),
    };
    let room_topic = match room() {
        Some(Ok(Some(r))) => r.topic,
        _ => None,
    };

    let message_list = match messages() {
        Some(Ok(list)) => list,
        Some(Err(e)) => {
            return rsx! {
                div { class: "flex-1 flex items-center justify-center text-red-500",
                    "Failed to load messages: {e}"
                }
            };
        }
        None => vec![],
    };

    // Determine mute state for composer
    let is_muted = u.is_muted;
    let mute_message = if is_muted {
        if let Some(ref until) = u.muted_until {
            format!("You are muted until {}", until)
        } else {
            "You are muted".to_string()
        }
    } else {
        String::new()
    };

    rsx! {
        // Header
        header { class: "px-6 py-3 border-b border-gray-200 bg-white",
            div { class: "flex items-baseline gap-3",
                h2 { class: "text-lg font-semibold text-gray-800", "# {room_name}" }
                if let Some(topic) = room_topic {
                    span { class: "text-sm text-gray-500", "{topic}" }
                }
            }
        }

        // Delete message confirmation
        if let Some(msg_id) = confirm_delete_msg() {
            div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
                div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                    h3 { class: "text-lg font-semibold text-gray-800 mb-2", "Delete Message" }
                    div { class: "space-y-3 mb-4",
                        label { class: "block text-sm font-medium text-gray-700", "Reason" }
                        input {
                            class: "w-full px-3 py-1.5 border border-gray-300 rounded text-sm",
                            r#type: "text",
                            placeholder: "Reason for deletion...",
                            value: "{delete_reason}",
                            oninput: move |e| delete_reason.set(e.value()),
                        }
                    }
                    div { class: "flex justify-end gap-2",
                        button {
                            class: "px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200",
                            onclick: move |_| {
                                confirm_delete_msg.set(None);
                                delete_reason.set(String::new());
                            },
                            "Cancel"
                        }
                        button {
                            class: "px-3 py-1.5 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700",
                            onclick: move |_| {
                                let reason = delete_reason();
                                spawn(async move {
                                    confirm_delete_msg.set(None);
                                    delete_reason.set(String::new());
                                    match delete_message(msg_id, reason).await {
                                        Ok(()) => {
                                            messages_version.set(messages_version() + 1);
                                        }
                                        Err(e) => {
                                            error.set(Some(format!("Delete failed: {}", e)));
                                        }
                                    }
                                });
                            },
                            "Delete"
                        }
                    }
                }
            }
        }

        // Message list
        div { class: "flex-1 overflow-y-auto px-6 py-4 space-y-3",
            if message_list.is_empty() {
                div { class: "text-center text-gray-400 mt-12",
                    "No messages yet — say hello!"
                }
            } else {
                for msg in message_list.iter() {
                    {
                        let msg_id = msg.id;
                        rsx! {
                            div { key: "{msg.id}", class: "group flex flex-col",
                                div { class: "flex items-baseline gap-2",
                                    span { class: "font-semibold text-gray-800", "{msg.author_name}" }
                                    span { class: "text-xs text-gray-400", "{msg.created_at}" }
                                    if is_mod {
                                        button {
                                            class: "opacity-0 group-hover:opacity-100 text-xs text-red-500 hover:text-red-700 ml-2 transition-opacity",
                                            onclick: move |_| {
                                                confirm_delete_msg.set(Some(msg_id));
                                            },
                                            "delete"
                                        }
                                    }
                                }
                                p { class: "text-gray-700 whitespace-pre-wrap", "{msg.body}" }
                            }
                        }
                    }
                }
            }
        }

        // Composer
        if is_muted {
            div { class: "px-6 py-3 border-t border-gray-200 bg-yellow-50 text-center",
                span { class: "text-sm text-yellow-700", "{mute_message}" }
            }
        } else {
            form {
                class: "px-6 py-3 border-t border-gray-200 bg-white",
                onsubmit: move |evt: Event<FormData>| {
                    evt.prevent_default();
                    let body = draft();
                    if body.trim().is_empty() {
                        return;
                    }
                    spawn(async move {
                        match send_message(parsed_id, body).await {
                            Ok(_) => {
                                draft.set(String::new());
                                error.set(None);
                                messages_version.set(messages_version() + 1);
                            }
                            Err(e) => {
                                error.set(Some(e.to_string()));
                            }
                        }
                    });
                },
                if let Some(err) = error() {
                    div { class: "mb-2 text-sm text-red-600", "{err}" }
                }
                div { class: "flex items-center gap-2",
                    input {
                        class: "flex-1 px-3 py-1.5 border border-gray-300 rounded",
                        r#type: "text",
                        placeholder: "Type a message…",
                        value: "{draft}",
                        oninput: move |evt| draft.set(evt.value()),
                    }
                    button {
                        class: "px-4 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700",
                        r#type: "submit",
                        "Send"
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check
```

- [ ] **Step 3: Run all tests**

```bash
docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test
```

- [ ] **Step 4: Commit**

```bash
git add src/components/room_view.rs
git commit -m "feat: add message delete button and mute banner in room view"
```
