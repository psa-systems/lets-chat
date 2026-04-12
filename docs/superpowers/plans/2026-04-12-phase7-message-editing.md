# Phase 7: Message Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow users to edit their own messages after sending. Admins and moderators can edit any message. Edits are broadcast in real time via WebSocket so all subscribers see the updated body without a page reload.

**Architecture:** A new `edited_at` column is added to `messages`. The `Message` model gains an `edited_at: Option<String>` field. A new `edit_message` server function validates ownership/role, writes the update, and broadcasts a `MessageEdited` WS event. Both `room_view` and `dm_view` handle the event to update the message in their local signal in place, and show an `(edited)` label. Inline editing is a `<textarea>` swap on the message row — no separate modal.

**Tech Stack:** No new dependencies. Builds via Docker (`rust:1.93-slim-trixie`).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `migrations/chat/0004_message_editing.sql` | Add `edited_at` column |
| Modify | `src/db/chat.rs` | Add `edited_at` to `RawMessage`; update `list_messages` query; add `get_message` and `update_message_body` |
| Modify | `src/models/message.rs` | Add `edited_at: Option<String>` to `Message` |
| Modify | `src/server_fns/chat.rs` | Update `list_messages` mapping; update `send_message`; add `edit_message` |
| Modify | `src/server_fns/dm.rs` | Update `send_dm_message` Message construction |
| Modify | `src/ws/events.rs` | Add `MessageEdited` variant to `ChatEvent` |
| Modify | `src/components/room_view.rs` | Inline edit UI + handle `MessageEdited` WS event |
| Modify | `src/components/dm_view.rs` | Same inline edit UI + handle `MessageEdited` WS event |
| Create | `tests/message_editing.rs` | Integration tests |

---

### Task 1: DB migration

**Files:**
- Create: `migrations/chat/0004_message_editing.sql`

- [ ] **Step 1: Create the migration file**

```sql
ALTER TABLE messages ADD COLUMN edited_at TEXT;
```

`edited_at IS NULL` means the message has never been edited. Follows the existing `deleted_at` / `deleted_by` pattern.

- [ ] **Step 2: Commit**

```bash
git add migrations/chat/0004_message_editing.sql
git commit -m "feat(db): add edited_at column to messages for Phase 7"
```

---

### Task 2: DB layer — `RawMessage` and new query functions

**Files:**
- Modify: `src/db/chat.rs`

- [ ] **Step 1: Add `edited_at` to `RawMessage` and update `list_messages`**

Replace the `RawMessage` struct and the `list_messages` function:

```rust
#[derive(Debug, Clone)]
pub struct RawMessage {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
}
```

```rust
pub async fn list_messages(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Vec<RawMessage>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at \
         FROM messages WHERE room_id = ? AND deleted_at IS NULL ORDER BY id ASC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| RawMessage {
            id: row.get("id"),
            room_id: row.get("room_id"),
            user_id: row.get("user_id"),
            body: row.get("body"),
            created_at: row.get("created_at"),
            edited_at: row.get("edited_at"),
        })
        .collect())
}
```

- [ ] **Step 2: Add `get_message` function**

Add after `list_messages`:

```rust
/// Fetch a single message by ID. Returns None if soft-deleted.
pub async fn get_message(
    pool: &sqlx::SqlitePool,
    message_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at \
         FROM messages WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| RawMessage {
        id: row.get("id"),
        room_id: row.get("room_id"),
        user_id: row.get("user_id"),
        body: row.get("body"),
        created_at: row.get("created_at"),
        edited_at: row.get("edited_at"),
    }))
}
```

- [ ] **Step 3: Add `update_message_body` function**

Add after `get_message`:

```rust
/// Update a message's body and set edited_at to now.
pub async fn update_message_body(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    new_body: &str,
) -> Result<String, sqlx::Error> {
    let edited_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "UPDATE messages SET body = ?, edited_at = ? WHERE id = ?",
    )
    .bind(new_body)
    .bind(&edited_at)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(edited_at)
}
```

- [ ] **Step 4: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20
```

Expected: compile errors in `src/server_fns/chat.rs` and `src/server_fns/dm.rs` because `Message` construction sites don't have `edited_at` yet. That is expected — fixed in Task 3.

- [ ] **Step 5: Commit**

```bash
git add src/db/chat.rs
git commit -m "feat(db): add get_message and update_message_body to chat DB layer"
```

---

### Task 3: Update `Message` model and all construction sites

**Files:**
- Modify: `src/models/message.rs`
- Modify: `src/server_fns/chat.rs`
- Modify: `src/server_fns/dm.rs`

- [ ] **Step 1: Add `edited_at` to `Message`**

Replace `src/models/message.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
}
```

- [ ] **Step 2: Update `list_messages` mapping in `src/server_fns/chat.rs`**

In the `list_messages` server function, update the `messages.push(Message { ... })` block to include `edited_at`:

```rust
messages.push(Message {
    id: rm.id,
    room_id: rm.room_id,
    user_id: rm.user_id,
    author_name,
    body: rm.body,
    created_at: rm.created_at,
    edited_at: rm.edited_at,
});
```

- [ ] **Step 3: Update `send_message` in `src/server_fns/chat.rs`**

In `send_message`, new messages have never been edited. Add `edited_at: None` to the `Message` construction inside the `ChatEvent::NewMessage` broadcast:

```rust
let event = crate::ws::events::ChatEvent::NewMessage {
    message: crate::models::Message {
        id: msg_id,
        room_id,
        user_id: user.id.clone(),
        author_name,
        body,
        created_at: now,
        edited_at: None,
    },
    is_dm: false,
};
```

- [ ] **Step 4: Update `send_dm_message` in `src/server_fns/dm.rs`**

Same change — add `edited_at: None` to the `Message` construction:

```rust
let event = crate::ws::events::ChatEvent::NewMessage {
    message: crate::models::Message {
        id: msg_id,
        room_id,
        user_id: user.id.clone(),
        author_name,
        body,
        created_at: now,
        edited_at: None,
    },
    is_dm: true,
};
```

- [ ] **Step 5: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20
```

Expected: clean compile (warnings OK).

- [ ] **Step 6: Commit**

```bash
git add src/models/message.rs src/server_fns/chat.rs src/server_fns/dm.rs
git commit -m "feat(models): add edited_at to Message model and update all construction sites"
```

---

### Task 4: `MessageEdited` WebSocket event

**Files:**
- Modify: `src/ws/events.rs`

- [ ] **Step 1: Add the `MessageEdited` variant**

Add to the `ChatEvent` enum in `src/ws/events.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    NewMessage {
        message: Message,
        is_dm: bool,
    },
    MessageEdited {
        message_id: i64,
        room_id: i64,
        new_body: String,
        edited_at: String,
    },
    MessageDeleted {
        message_id: i64,
        room_id: i64,
    },
    UserMuted {
        user_id: String,
        muted_until: Option<String>,
    },
    UserBanned {
        user_id: String,
    },
    UserKicked {
        user_id: String,
        room_id: i64,
    },
}
```

- [ ] **Step 2: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20
```

Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/ws/events.rs
git commit -m "feat(ws): add MessageEdited event variant"
```

---

### Task 5: `edit_message` server function

**Files:**
- Modify: `src/server_fns/chat.rs`

- [ ] **Step 1: Add `edit_message` to `src/server_fns/chat.rs`**

Add after `send_message`:

```rust
#[server]
pub async fn edit_message(message_id: i64, new_body: String) -> Result<(), ServerFnError> {
    let new_body = new_body.trim().to_string();
    if new_body.is_empty() {
        return Err(ServerFnError::new("Message body cannot be empty"));
    }

    let user = crate::server_fns::helpers::require_auth().await?;

    // Enforce max_message_length from settings
    let settings_pool = crate::db::get_settings_pool().await;
    if let Ok(Some(max_str)) = crate::db::settings::get_setting(settings_pool, "max_message_length").await {
        if let Ok(max_len) = max_str.parse::<usize>() {
            if new_body.len() > max_len {
                return Err(ServerFnError::new(format!(
                    "Message exceeds maximum length of {} characters",
                    max_len
                )));
            }
        }
    }

    let chat_pool = crate::db::get_chat_pool().await;

    // Fetch the message — returns None if soft-deleted
    let msg = crate::db::chat::get_message(chat_pool, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Message not found"))?;

    // Ownership check: must be author OR admin/moderator
    let is_owner = msg.user_id == user.id;
    let is_privileged = crate::server_fns::helpers::role_level(&user.role) >= 2;
    if !is_owner && !is_privileged {
        return Err(ServerFnError::new("Cannot edit another user's message"));
    }

    let edited_at = crate::db::chat::update_message_body(chat_pool, message_id, &new_body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Broadcast the edit to all subscribers of this room
    let event = crate::ws::events::ChatEvent::MessageEdited {
        message_id,
        room_id: msg.room_id,
        new_body,
        edited_at,
    };
    crate::ws::hub::get_hub().broadcast_to_room(msg.room_id, &event);

    Ok(())
}
```

- [ ] **Step 2: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20
```

Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/server_fns/chat.rs
git commit -m "feat(server): add edit_message server function with ownership and role checks"
```

---

### Task 6: UI — `room_view.rs`

**Files:**
- Modify: `src/components/room_view.rs`

- [ ] **Step 1: Add edit signals and import**

Add to the imports at the top of `room_view.rs`:

```rust
use crate::server_fns::chat::{edit_message, get_room, list_messages, send_message};
```

Add two new signals alongside the existing `draft`, `error`, `delete_reason`, and `confirm_delete_msg`:

```rust
let mut editing_msg_id = use_signal(|| Option::<i64>::None);
let mut edit_draft = use_signal(String::new);
let mut edit_error = use_signal(|| Option::<String>::None);
```

- [ ] **Step 2: Handle `MessageEdited` in the WS `use_effect`**

In the existing WS `use_effect` block, add an arm for `MessageEdited`:

```rust
use_effect(move || {
    if let Some(ref event) = *ws.latest_event.read() {
        match event {
            ChatEvent::NewMessage { message, .. } if message.room_id == parsed_id => {
                messages_version.set(messages_version() + 1);
            }
            ChatEvent::MessageDeleted { room_id, .. } if *room_id == parsed_id => {
                messages_version.set(messages_version() + 1);
            }
            ChatEvent::MessageEdited { room_id, .. } if *room_id == parsed_id => {
                messages_version.set(messages_version() + 1);
            }
            _ => {}
        }
    }
});
```

- [ ] **Step 3: Add edit button and inline edit form to the message render loop**

Replace the `rsx!` block inside `for msg in message_list.iter()` with the following. The key additions are: an "Edit" button (visible on hover, only for the message owner), and an inline edit form that replaces the body text when `editing_msg_id == Some(msg.id)`.

```rust
for msg in message_list.iter() {
    {
        let msg_id = msg.id;
        let msg_user_id = msg.user_id.clone();
        let msg_body = msg.body.clone();
        let is_own = msg_user_id == u.id;
        let is_editing = editing_msg_id() == Some(msg_id);
        rsx! {
            div { key: "{msg.id}", class: "group flex flex-col",
                div { class: "flex items-baseline gap-2",
                    if msg_user_id != u.id {
                        Link {
                            to: Route::Dm { user_id: msg_user_id.clone() },
                            class: "font-semibold text-gray-800 hover:underline hover:text-blue-600",
                            "{msg.author_name}"
                        }
                    } else {
                        span { class: "font-semibold text-gray-800", "{msg.author_name}" }
                    }
                    span { class: "text-xs text-gray-400", "{msg.created_at}" }
                    if msg.edited_at.is_some() {
                        span { class: "text-xs text-gray-400 italic", "(edited)" }
                    }
                    // Edit button — own messages only
                    if is_own && !is_editing {
                        button {
                            class: "opacity-0 group-hover:opacity-100 text-xs text-blue-500 hover:text-blue-700 ml-2 transition-opacity",
                            onclick: move |_| {
                                editing_msg_id.set(Some(msg_id));
                                edit_draft.set(msg_body.clone());
                                edit_error.set(None);
                            },
                            "edit"
                        }
                    }
                    if is_mod && !is_editing {
                        button {
                            class: "opacity-0 group-hover:opacity-100 text-xs text-red-500 hover:text-red-700 ml-2 transition-opacity",
                            onclick: move |_| {
                                confirm_delete_msg.set(Some(msg_id));
                            },
                            "delete"
                        }
                    }
                }
                // Inline edit form or normal body
                if is_editing {
                    div { class: "mt-1 flex flex-col gap-1",
                        if let Some(err) = edit_error() {
                            div { class: "text-xs text-red-600", "{err}" }
                        }
                        textarea {
                            class: "w-full px-3 py-1.5 border border-blue-400 rounded text-sm resize-none",
                            rows: "3",
                            value: "{edit_draft}",
                            oninput: move |e| edit_draft.set(e.value()),
                        }
                        div { class: "flex gap-2",
                            button {
                                class: "px-3 py-1 text-xs font-medium text-white bg-blue-600 rounded hover:bg-blue-700",
                                onclick: move |_| {
                                    let body = edit_draft();
                                    spawn(async move {
                                        match edit_message(msg_id, body).await {
                                            Ok(()) => {
                                                editing_msg_id.set(None);
                                                edit_draft.set(String::new());
                                                edit_error.set(None);
                                                messages_version.set(messages_version() + 1);
                                            }
                                            Err(e) => {
                                                edit_error.set(Some(e.to_string()));
                                            }
                                        }
                                    });
                                },
                                "Save"
                            }
                            button {
                                class: "px-3 py-1 text-xs font-medium text-gray-700 bg-gray-100 rounded hover:bg-gray-200",
                                onclick: move |_| {
                                    editing_msg_id.set(None);
                                    edit_draft.set(String::new());
                                    edit_error.set(None);
                                },
                                "Cancel"
                            }
                        }
                    }
                } else {
                    p { class: "text-gray-700 whitespace-pre-wrap", "{msg.body}" }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20
```

Expected: clean compile.

- [ ] **Step 5: Commit**

```bash
git add src/components/room_view.rs
git commit -m "feat(ui): add inline message editing to room view"
```

---

### Task 7: UI — `dm_view.rs`

**Files:**
- Modify: `src/components/dm_view.rs`

Apply the identical changes from Task 6 to `dm_view.rs`:

- [ ] **Step 1: Add `edit_message` to the import from `server_fns::chat`**

```rust
use crate::server_fns::chat::{edit_message, list_messages};
```

- [ ] **Step 2: Add `editing_msg_id`, `edit_draft`, `edit_error` signals**

Same three signals as Task 6 Step 1.

- [ ] **Step 3: Add `MessageEdited` arm to the WS `use_effect`**

Same arm as Task 6 Step 2, matching on `room_id == room_id` (the DM room's ID).

- [ ] **Step 4: Update the message render loop**

Same pattern as Task 6 Step 3. DMs don't have a mod delete button, so omit the `is_mod` check — only show the edit button for `is_own`.

- [ ] **Step 5: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20
```

Expected: clean compile.

Also run the WASM target check:

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie \
  cargo check --target wasm32-unknown-unknown 2>&1 | tail -20
```

Expected: clean compile.

- [ ] **Step 6: Commit**

```bash
git add src/components/dm_view.rs
git commit -m "feat(ui): add inline message editing to DM view"
```

---

### Task 8: Integration tests

**Files:**
- Create: `tests/message_editing.rs`

- [ ] **Step 1: Write the test file**

```rust
use lets_chat::db;

async fn setup() -> (sqlx::SqlitePool, sqlx::SqlitePool) {
    let chat_pool = db::create_chat_pool("sqlite::memory:").await.unwrap();
    let auth_pool = db::create_auth_pool("sqlite::memory:").await.unwrap();
    (chat_pool, auth_pool)
}

#[tokio::test]
async fn test_edit_message_updates_body_and_sets_edited_at() {
    let (chat_pool, _auth_pool) = setup().await;

    // Create a room and message
    let room_id = db::chat::create_room(&chat_pool, "test", None).await.unwrap();
    let msg_id = db::chat::insert_message(&chat_pool, room_id, "user-1", "original body").await.unwrap();

    // Edit the message
    let edited_at = db::chat::update_message_body(&chat_pool, msg_id, "edited body").await.unwrap();

    // Verify
    let msg = db::chat::get_message(&chat_pool, msg_id).await.unwrap().unwrap();
    assert_eq!(msg.body, "edited body");
    assert_eq!(msg.edited_at, Some(edited_at));
}

#[tokio::test]
async fn test_get_message_returns_none_for_soft_deleted() {
    let (chat_pool, _auth_pool) = setup().await;

    let room_id = db::chat::create_room(&chat_pool, "test2", None).await.unwrap();
    let msg_id = db::chat::insert_message(&chat_pool, room_id, "user-1", "hello").await.unwrap();

    // Soft-delete the message directly
    sqlx::query("UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'user-1' WHERE id = ?")
        .bind(msg_id)
        .execute(&chat_pool)
        .await
        .unwrap();

    // get_message must return None
    let result = db::chat::get_message(&chat_pool, msg_id).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_list_messages_includes_edited_at() {
    let (chat_pool, _auth_pool) = setup().await;

    let room_id = db::chat::create_room(&chat_pool, "test3", None).await.unwrap();
    let msg_id = db::chat::insert_message(&chat_pool, room_id, "user-1", "first").await.unwrap();

    // Not yet edited — edited_at should be None
    let messages = db::chat::list_messages(&chat_pool, room_id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].edited_at.is_none());

    // Edit it
    db::chat::update_message_body(&chat_pool, msg_id, "updated").await.unwrap();

    // Now edited_at should be Some
    let messages = db::chat::list_messages(&chat_pool, room_id).await.unwrap();
    assert!(messages[0].edited_at.is_some());
    assert_eq!(messages[0].body, "updated");
}
```

- [ ] **Step 2: Run tests**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie \
  cargo test --test message_editing 2>&1 | tail -30
```

Expected: 3 tests pass.

- [ ] **Step 3: Run full test suite to check for regressions**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie \
  cargo test 2>&1 | tail -30
```

Expected: all existing tests plus the 3 new ones pass.

- [ ] **Step 4: Commit**

```bash
git add tests/message_editing.rs
git commit -m "test: add integration tests for message editing DB layer"
```

---

## Phase 7 complete checklist

- [ ] Migration `0004_message_editing.sql` created
- [ ] `RawMessage` has `edited_at`; `list_messages` selects it
- [ ] `get_message` and `update_message_body` added to `src/db/chat.rs`
- [ ] `Message` model has `edited_at: Option<String>`
- [ ] All `Message` construction sites updated (`send_message`, `send_dm_message`, `list_messages`)
- [ ] `ChatEvent::MessageEdited` variant added
- [ ] `edit_message` server function added with ownership + role check
- [ ] `room_view.rs` has inline edit UI and handles `MessageEdited`
- [ ] `dm_view.rs` has inline edit UI and handles `MessageEdited`
- [ ] 3 integration tests pass; full suite clean
- [ ] `cargo check --target wasm32-unknown-unknown` passes
