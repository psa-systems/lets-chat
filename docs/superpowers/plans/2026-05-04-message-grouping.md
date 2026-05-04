# Message Grouping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Group consecutive messages from the same author within a 5-minute window into a single visual block, hiding the username/timestamp header on follow-ups while preserving full per-message functionality.

**Architecture:** Compute an `is_follow_up: bool` flag at render time (no schema change). Loaders compute the flag in a single chronological pass; HTTP POST and WS rendering paths query the immediately-prior message to compute the flag for a freshly inserted message. On delete of a header message, look up the next message and broadcast a regrouping event so its `is_follow_up = false` rendering replaces the orphaned follow-up.

**Tech Stack:** Rust, Axum 0.8, Askama templates, SQLx (SQLite), HTMX with `hx-swap-oob`.

**Spec:** `docs/superpowers/specs/2026-05-04-message-grouping-design.md`

**Branch:** `feat/message-grouping` (already created and contains the committed spec).

---

## File Map

**Modified:**
- `server/src/views/room.rs` - add `is_follow_up: bool` to `MessageView`.
- `server/src/db/chat.rs` - add `MESSAGE_GROUPING_WINDOW`, `is_follow_up_of`, `prior_message_in_room`, `next_message_in_room`.
- `server/src/routes/room.rs` - compute flag on GET, POST, edit, single-message, delete promote.
- `server/src/routes/dm.rs` - compute flag on GET.
- `server/src/routes/ws.rs` - compute flag in `render_new_message` and `render_edited_message`.
- `server/src/ws/events.rs` - add `ChatEvent::MessageRegrouped { message_id, room_id }` variant.
- `server/src/views/ws_fragments.rs` - route `MessageRegrouped` through the existing edited-message render path.
- `server/templates/room/message.html` - conditional header render, hover overlay for edit/delete, conditional padding.

**Created (test files):**
- `server/tests/message_grouping.rs` - unit tests for grouping logic and integration tests for routes.

---

## Task 1: Add the grouping constant and pure-function helper

Establish the threshold and the standalone follow-up predicate. Pure logic, no DB - lets the unit tests run without a pool.

**Files:**
- Modify: `server/src/db/chat.rs` (top of file, after imports)
- Test: `server/tests/message_grouping.rs` (new file)

- [ ] **Step 1: Write the failing tests for the predicate**

Create `server/tests/message_grouping.rs`:

```rust
use lets_chat::db::chat::{is_follow_up_of, MESSAGE_GROUPING_WINDOW};

fn ts(s: &str) -> String {
    s.to_string()
}

#[test]
fn follow_up_when_same_user_within_window() {
    assert!(is_follow_up_of(
        Some(("alice", &ts("2026-05-04 12:00:00"))),
        ("alice", &ts("2026-05-04 12:00:30")),
    ));
}

#[test]
fn not_follow_up_when_different_user() {
    assert!(!is_follow_up_of(
        Some(("alice", &ts("2026-05-04 12:00:00"))),
        ("bob", &ts("2026-05-04 12:00:30")),
    ));
}

#[test]
fn not_follow_up_when_gap_exceeds_window() {
    assert!(!is_follow_up_of(
        Some(("alice", &ts("2026-05-04 12:00:00"))),
        ("alice", &ts("2026-05-04 12:06:00")),
    ));
}

#[test]
fn follow_up_at_exact_window_boundary() {
    // 5 minutes inclusive
    assert!(is_follow_up_of(
        Some(("alice", &ts("2026-05-04 12:00:00"))),
        ("alice", &ts("2026-05-04 12:05:00")),
    ));
}

#[test]
fn not_follow_up_when_no_prior() {
    assert!(!is_follow_up_of(
        None,
        ("alice", &ts("2026-05-04 12:00:00")),
    ));
}

#[test]
fn window_is_five_minutes() {
    assert_eq!(MESSAGE_GROUPING_WINDOW.num_seconds(), 300);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `./dev/cargo test -p lets-chat-server --test message_grouping`

Expected: compilation failure - `MESSAGE_GROUPING_WINDOW` and `is_follow_up_of` are not defined.

- [ ] **Step 3: Add the constant and predicate to `server/src/db/chat.rs`**

Add the following near the top of `server/src/db/chat.rs`, after the existing `use` statements:

```rust
/// Two messages from the same author within this window are visually grouped:
/// the second is rendered as a "follow-up" (no username/timestamp header).
pub const MESSAGE_GROUPING_WINDOW: chrono::Duration = chrono::Duration::minutes(5);

/// Pure predicate: would `(curr_user, curr_created_at)` render as a follow-up
/// of the immediately-prior message `(prev_user, prev_created_at)`?
///
/// Times are SQLite "YYYY-MM-DD HH:MM:SS" UTC strings. Returns `false` when
/// `prior` is `None` (first message in the thread).
pub fn is_follow_up_of(
    prior: Option<(&str, &str)>,
    curr: (&str, &str),
) -> bool {
    let Some((prev_user, prev_at)) = prior else {
        return false;
    };
    if prev_user != curr.0 {
        return false;
    }
    let fmt = "%Y-%m-%d %H:%M:%S";
    let Ok(prev_dt) = chrono::NaiveDateTime::parse_from_str(prev_at, fmt) else {
        return false;
    };
    let Ok(curr_dt) = chrono::NaiveDateTime::parse_from_str(curr.1, fmt) else {
        return false;
    };
    let delta = curr_dt - prev_dt;
    delta >= chrono::Duration::zero() && delta <= MESSAGE_GROUPING_WINDOW
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `./dev/cargo test -p lets-chat-server --test message_grouping`

Expected: all 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add server/src/db/chat.rs server/tests/message_grouping.rs
git commit -m "feat(grouping): add MESSAGE_GROUPING_WINDOW constant and is_follow_up_of predicate"
```

---

## Task 2: Add `is_follow_up` to `MessageView` and pipe it through

Add the field and default it to `false` everywhere it is constructed. Render template stays unchanged for now - we wire the visual change in Task 4 once the field is populated.

**Files:**
- Modify: `server/src/views/room.rs:6-20`
- Modify: `server/src/routes/room.rs` - all `MessageView { ... }` literals
- Modify: `server/src/routes/dm.rs` - one `MessageView { ... }` literal
- Modify: `server/src/routes/ws.rs` - both `MessageView { ... }` literals

- [ ] **Step 1: Add the field**

Edit `server/src/views/room.rs`. The current struct:

```rust
pub struct MessageView {
    pub id: i64,
    pub user_id: String,
    pub username: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub body: String,
    pub reactions: Vec<ReactionView>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub viewer_id: String,
    /// HH:MM peer-read timestamp shown under this message in a DM, or None.
    /// Only one own-authored message in a DM should have this set at a time.
    pub seen_caption: Option<String>,
}
```

Add `is_follow_up`:

```rust
pub struct MessageView {
    pub id: i64,
    pub user_id: String,
    pub username: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub body: String,
    pub reactions: Vec<ReactionView>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub viewer_id: String,
    /// HH:MM peer-read timestamp shown under this message in a DM, or None.
    /// Only one own-authored message in a DM should have this set at a time.
    pub seen_caption: Option<String>,
    /// True when this message follows another message from the same author
    /// within MESSAGE_GROUPING_WINDOW. Hides the username/timestamp header.
    pub is_follow_up: bool,
}
```

- [ ] **Step 2: Run the build to surface every construction site**

Run: `./dev/cargo check -p lets-chat-server`

Expected: compile errors at every `MessageView { ... }` literal that does not include `is_follow_up`. Note each location.

- [ ] **Step 3: Add `is_follow_up: false` to every literal**

Edit each location below, adding `is_follow_up: false,` as the final field. (Real values are filled in by Task 3-5; this step keeps the build green.)

In `server/src/routes/room.rs`:

- The literal in `get_room` (around line 82-94):
  ```rust
  messages.push(MessageView {
      id: m.id,
      user_id: m.user_id.clone(),
      username,
      created_at: m.created_at,
      edited_at: m.edited_at,
      body: m.body,
      reactions,
      can_edit,
      can_delete,
      viewer_id: user.id.clone(),
      seen_caption: None,
      is_follow_up: false,
  });
  ```

- The literal in `get_single_message` (around line 240-252): add `is_follow_up: false,` as the final field.
- The literal in `patch_message` (around line 303-315): add `is_follow_up: false,` as the final field.

In `server/src/routes/dm.rs`, the literal in `get_dm` (around line 101-113): add `is_follow_up: false,` as the final field.

In `server/src/routes/ws.rs`:

- The literal in `render_new_message` (around line 358-370): add `is_follow_up: false,` as the final field.
- The literal in `render_edited_message` (around line 399-411): add `is_follow_up: false,` as the final field.

- [ ] **Step 4: Verify the build is green again**

Run: `./dev/cargo check -p lets-chat-server`

Expected: success.

- [ ] **Step 5: Run the existing test suite to confirm no regression**

Run: `./dev/cargo test -p lets-chat-server`

Expected: all existing tests pass. The new `message_grouping` tests still pass.

- [ ] **Step 6: Commit**

```bash
git add server/src/views/room.rs server/src/routes/room.rs server/src/routes/dm.rs server/src/routes/ws.rs
git commit -m "feat(grouping): add is_follow_up field to MessageView (defaulted to false)"
```

---

## Task 3: Compute `is_follow_up` on initial page load

Walk loaded messages chronologically with a running prev-pointer and set the flag. Also exposes a private DB helper that the POST and WS paths use in later tasks.

**Files:**
- Modify: `server/src/db/chat.rs` - add `prior_message_in_room`.
- Modify: `server/src/routes/room.rs:67-95` - room page loader.
- Modify: `server/src/routes/dm.rs:78-114` - DM page loader.
- Test: `server/tests/message_grouping.rs` - add page-load integration tests.

- [ ] **Step 1: Write a failing integration test for the room page**

Append to `server/tests/message_grouping.rs`:

```rust
mod page_grouping {
    use lets_chat::db;
    use lets_chat::state::AppState;
    use sqlx::SqlitePool;

    async fn setup_pools() -> (SqlitePool, SqlitePool, SqlitePool) {
        let auth = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let chat = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let settings = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations/auth").run(&auth).await.unwrap();
        sqlx::migrate!("./migrations/chat").run(&chat).await.unwrap();
        sqlx::migrate!("./migrations/settings").run(&settings).await.unwrap();
        (auth, chat, settings)
    }

    #[tokio::test]
    async fn loader_marks_consecutive_same_user_as_follow_ups() {
        let (auth, chat, _settings) = setup_pools().await;

        // Create a user and a public room.
        let user_id = db::auth::create_user(&auth, "alice", "x", false).await.unwrap();
        let room_id = db::chat::create_room(&chat, "general", None, "public", None).await.unwrap();

        // Insert 3 messages from the same user back-to-back. SQLite's default
        // CURRENT_TIMESTAMP has 1-second resolution, but the inserts run in
        // sequence so prior < curr by at least 1ms in wall-clock terms; the
        // grouping predicate uses second resolution, so identical seconds also
        // satisfy delta <= 5min.
        let _id1 = db::chat::insert_message(&chat, room_id, &user_id, "first").await.unwrap();
        let _id2 = db::chat::insert_message(&chat, room_id, &user_id, "second").await.unwrap();
        let _id3 = db::chat::insert_message(&chat, room_id, &user_id, "third").await.unwrap();

        // Load and run the same chronological pass that get_room performs.
        let raw = db::chat::list_messages(&chat, room_id).await.unwrap();
        let mut prev: Option<(String, String)> = None;
        let mut flags: Vec<bool> = Vec::new();
        for m in &raw {
            let is_fu = db::chat::is_follow_up_of(
                prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
                (&m.user_id, &m.created_at),
            );
            prev = Some((m.user_id.clone(), m.created_at.clone()));
            flags.push(is_fu);
        }

        assert_eq!(flags, vec![false, true, true]);
    }
}
```

The test calls `db::auth::create_user`. Confirm the signature in `server/src/db/auth.rs` matches `(pool, username, password_or_hash, is_first)` - if it differs, adapt the call. The point is: insert one user, one room, three messages.

- [ ] **Step 2: Run the test to verify it fails**

Run: `./dev/cargo test -p lets-chat-server --test message_grouping page_grouping`

Expected: PASS already, since `is_follow_up_of` was added in Task 1 and the test does not depend on `MessageView`. If the test compiles and passes, that is correct - this test pins the behavior we will use in production. Move on to Step 3 to wire it into the loaders.

If the test does NOT pass, debug `is_follow_up_of` before continuing.

- [ ] **Step 3: Update the room loader to set `is_follow_up`**

Edit `server/src/routes/room.rs` `get_room`. Replace the message-construction loop (around line 67-95) with:

```rust
let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
let mut prev: Option<(String, String)> = None;
for m in raw_messages {
    let username = if let Some(name) = username_cache.get(&m.user_id) {
        name.clone()
    } else {
        let resolved = db::auth::find_user_by_id(&state.auth, &m.user_id)
            .await?
            .map(|r| r.username)
            .unwrap_or_else(|| "(unknown)".to_string());
        username_cache.insert(m.user_id.clone(), resolved.clone());
        resolved
    };
    let can_edit = m.user_id == user.id;
    let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
    let reactions = reactions_by_message.remove(&m.id).unwrap_or_default();
    let is_follow_up = db::chat::is_follow_up_of(
        prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
        (&m.user_id, &m.created_at),
    );
    prev = Some((m.user_id.clone(), m.created_at.clone()));
    messages.push(MessageView {
        id: m.id,
        user_id: m.user_id.clone(),
        username,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: user.id.clone(),
        seen_caption: None,
        is_follow_up,
    });
}
```

- [ ] **Step 4: Update the DM loader to set `is_follow_up`**

Edit `server/src/routes/dm.rs` `get_dm`. Replace the message-construction loop (around line 78-114) with:

```rust
let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
let mut prev: Option<(String, String)> = None;
for m in raw_messages {
    let username = if let Some(name) = username_cache.get(&m.user_id) {
        name.clone()
    } else {
        let resolved = db::auth::find_user_by_id(&state.auth, &m.user_id)
            .await?
            .map(|r| r.username)
            .unwrap_or_else(|| "(unknown)".to_string());
        username_cache.insert(m.user_id.clone(), resolved.clone());
        resolved
    };
    let can_edit = m.user_id == user.id;
    let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
    let reactions: Vec<ReactionView> = db::chat::list_reactions(&state.chat, m.id, &user.id)
        .await?
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let is_follow_up = db::chat::is_follow_up_of(
        prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
        (&m.user_id, &m.created_at),
    );
    prev = Some((m.user_id.clone(), m.created_at.clone()));
    messages.push(MessageView {
        id: m.id,
        user_id: m.user_id.clone(),
        username,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: user.id.clone(),
        seen_caption: None,
        is_follow_up,
    });
}
```

- [ ] **Step 5: Add the prior-message DB helper for use in later tasks**

Append to `server/src/db/chat.rs`:

```rust
/// Fetch the most recent non-deleted message in `room_id` strictly before
/// `before_id` (by id). Returns `None` if `before_id` is the first message in
/// the room. Used to compute `is_follow_up` for a message rendered in
/// isolation (POST handler and WS new-message broadcast).
pub async fn prior_message_in_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    before_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at \
         FROM messages \
         WHERE room_id = ? AND id < ? AND deleted_at IS NULL \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(room_id)
    .bind(before_id)
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

/// Fetch the next non-deleted message in `room_id` strictly after `after_id`
/// (by id). Returns `None` if `after_id` is the last message in the room.
/// Used by the delete handler to repair grouping when a header is removed.
pub async fn next_message_in_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    after_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at \
         FROM messages \
         WHERE room_id = ? AND id > ? AND deleted_at IS NULL \
         ORDER BY id ASC LIMIT 1",
    )
    .bind(room_id)
    .bind(after_id)
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

- [ ] **Step 6: Run the full test suite**

Run: `./dev/cargo test -p lets-chat-server`

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add server/src/db/chat.rs server/src/routes/room.rs server/src/routes/dm.rs server/tests/message_grouping.rs
git commit -m "feat(grouping): compute is_follow_up on page load and add prior/next helpers"
```

---

## Task 4: Render the grouped layout in `room/message.html`

Now that the field is populated correctly on page load, render the visual change. Hide the header on follow-ups, move edit/delete to a hover overlay so they remain reachable on follow-ups, reduce vertical padding on follow-ups.

**Files:**
- Modify: `server/templates/room/message.html`

- [ ] **Step 1: Replace the template with the grouped layout**

Overwrite `server/templates/room/message.html` with:

```html
<div id="msg-{{ message.id }}"{% if oob %} hx-swap-oob="outerHTML"{% endif %} class="relative px-4 {% if message.is_follow_up %}py-0.5{% else %}py-2{% endif %} hover:bg-slate-100 group">
  {% if !message.is_follow_up %}
  <div class="flex items-baseline gap-2">
    {% if message.user_id != message.viewer_id %}
    <a href="/dm/{{ message.user_id }}" class="font-medium text-blue-700 hover:underline" title="Direct message {{ message.username }}">{{ message.username }}</a>
    {% else %}
    <span class="font-medium">{{ message.username }}</span>
    {% endif %}
    <span class="text-xs text-slate-500">{{ message.created_at }}</span>
    {% if message.edited_at.is_some() %}
    <span class="text-xs text-slate-400">(edited)</span>
    {% endif %}
  </div>
  {% endif %}
  {% if message.can_edit || message.can_delete %}
  <span class="absolute right-2 top-1 opacity-0 group-hover:opacity-100 flex gap-2 text-xs bg-slate-100 px-1 rounded">
    {% if message.can_edit %}
    <button hx-get="/messages/{{ message.id }}/edit" hx-target="#msg-{{ message.id }}" hx-swap="outerHTML" class="text-blue-600 hover:underline">Edit</button>
    {% endif %}
    {% if message.can_delete %}
    <button hx-delete="/messages/{{ message.id }}" hx-target="#msg-{{ message.id }}" hx-swap="outerHTML" hx-confirm="Delete this message?" class="text-red-600 hover:underline">Delete</button>
    {% endif %}
  </span>
  {% endif %}
  <div class="whitespace-pre-wrap">{{ message.body }}</div>
  <div id="reactions-{{ message.id }}" class="mt-1">
    {% let message_id = message.id %}
    {% let reactions = message.reactions.as_slice() %}
    {% include "partials/reaction_bar.html" %}
  </div>
  {% if message.user_id == message.viewer_id %}
  <div id="seen-{{ message.id }}">
    {% if let Some(t) = message.seen_caption.as_ref() %}
    <div class="text-xs text-slate-400">Seen {{ t }}</div>
    {% endif %}
  </div>
  {% endif %}
</div>
```

Key changes from the current template:

1. Outer `<div>` adds `relative` (for the hover overlay positioning) and uses conditional padding (`py-0.5` on follow-ups, `py-2` otherwise).
2. The header `<div class="flex items-baseline gap-2">` is wrapped in `{% if !message.is_follow_up %}...{% endif %}`.
3. Edit/delete buttons are extracted from inside the header into an absolutely-positioned overlay so they appear on every message (not only headers) and are revealed on hover via `group-hover:opacity-100`.

- [ ] **Step 2: Rebuild Tailwind so the new utility classes (`opacity-0`, `group-hover:opacity-100`, `bg-slate-100`, `rounded`, `py-0.5`, `relative`, `top-1`, `right-2`, `px-1`) are present in the built CSS**

Run: `just build-css`

Expected: success (no error). The output `server/assets/tailwind-built.css` is regenerated with the new classes.

- [ ] **Step 3: Run the build and existing tests to confirm no regression**

Run: `./dev/cargo test -p lets-chat-server`

Expected: all tests pass.

- [ ] **Step 4: Manually verify the rendering**

This is a UI change - run the dev server and exercise the feature.

Run: `just dev-web-local`

In a browser at `http://localhost:18080`:
1. Log in (register the first account if needed - it auto-promotes to admin).
2. Open the default public room.
3. Send three messages back-to-back from the same account.
4. Verify only the first shows the username and timestamp; the next two show body only with reduced spacing.
5. Hover over a follow-up message: the Edit/Delete buttons appear in the top-right corner.
6. Verify reactions still render per message (click a reaction; it appears on that specific message).
7. Open a second browser session as a different user, send a message into the same room from that account, then send another message from the first account: the first account's new message renders as a header (the chain was broken by the second user).

Note: live WS-pushed messages will all render as headers at this stage (Task 5 wires up the WS path). Page-reload after sending will show correct grouping; that is enough to verify the template change in isolation.

Stop the dev server: `just dev-web-down` (or Ctrl-C if foreground).

- [ ] **Step 5: Commit**

```bash
git add server/templates/room/message.html server/assets/tailwind-built.css
git commit -m "feat(grouping): hide header and tighten spacing on follow-up messages"
```

If `tailwind-built.css` is gitignored (per the CLAUDE.md note), drop it from the staged files: `git restore --staged server/assets/tailwind-built.css`. The build pipeline regenerates it.

---

## Task 5: Compute `is_follow_up` for new and edited messages over WebSocket

The POST handler renders no message fragment of its own (it returns the composer fragment); the WS handler is the single source of message-fragment HTML for newly broadcast messages. Wire the prior-message lookup into `render_new_message` and `render_edited_message`.

**Files:**
- Modify: `server/src/routes/ws.rs:350-413` - `render_new_message` and `render_edited_message`.

- [ ] **Step 1: Update `render_new_message` to look up the prior message and compute the flag**

Replace the function body in `server/src/routes/ws.rs` (around line 350-372):

```rust
async fn render_new_message(
    state: &AppState,
    message: &models::Message,
    viewer: &User,
) -> Option<String> {
    let can_edit = message.user_id == viewer.id;
    let can_delete =
        message.user_id == viewer.id || viewer.role == "admin" || viewer.role == "moderator";
    let prior = db::chat::prior_message_in_room(&state.chat, message.room_id, message.id)
        .await
        .ok()
        .flatten();
    let is_follow_up = db::chat::is_follow_up_of(
        prior.as_ref().map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (message.user_id.as_str(), message.created_at.as_str()),
    );
    let view = MessageView {
        id: message.id,
        user_id: message.user_id.clone(),
        username: message.author_name.clone(),
        created_at: message.created_at.clone(),
        edited_at: message.edited_at.clone(),
        body: message.body.clone(),
        reactions: Vec::new(),
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
    };
    NewMessageFragment { message: &view }.render().ok()
}
```

The signature already takes `state` (it was previously named `_state` and ignored). Rename the parameter from `_state` to `state` so the function compiles.

- [ ] **Step 2: Update `render_edited_message` similarly**

Replace the function body (around line 377-413). The change is identical: look up the prior message and compute the flag before constructing `MessageView`.

```rust
async fn render_edited_message(state: &AppState, message_id: i64, viewer: &User) -> Option<String> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await
        .ok()??;
    let username = db::auth::find_user_by_id(&state.auth, &m.user_id)
        .await
        .ok()?
        .map(|u| u.username)
        .unwrap_or_else(|| "(unknown)".to_string());
    let counts = db::chat::list_reactions(&state.chat, m.id, &viewer.id)
        .await
        .ok()?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id)
        .await
        .ok()
        .flatten();
    let is_follow_up = db::chat::is_follow_up_of(
        prior.as_ref().map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (m.user_id.as_str(), m.created_at.as_str()),
    );
    let can_edit = m.user_id == viewer.id;
    let can_delete = m.user_id == viewer.id || viewer.role == "admin" || viewer.role == "moderator";
    let view = MessageView {
        id: m.id,
        user_id: m.user_id,
        username,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
    };
    EditedMessageFragment { message: &view }.render().ok()
}
```

- [ ] **Step 3: Update `get_single_message` and `patch_message` in `server/src/routes/room.rs` to also compute `is_follow_up`**

Both handlers render a single `MessageView` and currently default `is_follow_up: false`. Use the same DB helper.

In `get_single_message` (around line 240-252), before constructing `MessageView`:

```rust
let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id)
    .await?;
let is_follow_up = db::chat::is_follow_up_of(
    prior.as_ref().map(|p| (p.user_id.as_str(), p.created_at.as_str())),
    (m.user_id.as_str(), m.created_at.as_str()),
);
```

Then change `is_follow_up: false,` to `is_follow_up,`.

In `patch_message` (around line 303-315), do the same. Place the lookup before the `MessageView { ... }` literal and replace the literal field.

- [ ] **Step 4: Run the test suite**

Run: `./dev/cargo test -p lets-chat-server`

Expected: all tests pass.

- [ ] **Step 5: Manually verify live grouping over WS**

Run: `just dev-web-local`

1. Open the same room in two browser windows logged in as different users (e.g., admin and a second account).
2. From the admin window, send three messages quickly.
3. Observe both windows: the second and third messages should render as follow-ups (no header) without a refresh.
4. From the second account, send a message: it renders as a header in both windows.
5. From admin, send another message: header (chain broken).

Stop the dev server.

- [ ] **Step 6: Commit**

```bash
git add server/src/routes/ws.rs server/src/routes/room.rs
git commit -m "feat(grouping): compute is_follow_up on WS new-message and edit broadcasts"
```

---

## Task 6: Promote-on-delete

When a header message is soft-deleted, the next message (if it was a follow-up of the deleted message within the window) becomes orphaned: it has no header above it because the header is gone. Promote it: re-render with `is_follow_up = false` and broadcast.

**Files:**
- Modify: `server/src/ws/events.rs` - add `MessageRegrouped { message_id, room_id }` variant.
- Modify: `server/src/views/ws_fragments.rs:82-102` - extend the no-op match arm.
- Modify: `server/src/routes/ws.rs` - dispatch `MessageRegrouped` to `render_edited_message` (which already re-fetches and applies the current grouping flag).
- Modify: `server/src/routes/room.rs:325-349` - `delete_message` handler: compute promote and broadcast.
- Test: `server/tests/message_grouping.rs` - add promote-on-delete test.

- [ ] **Step 1: Add the new event variant**

Edit `server/src/ws/events.rs`. Add a new variant inside the `ChatEvent` enum (place it next to `MessageEdited`):

```rust
/// Emitted by the delete handler when removing a header message exposes a
/// follow-up that should be promoted to a header. Recipients re-render the
/// referenced message with the current grouping flag (which will now be
/// `false` because the prior message no longer exists).
MessageRegrouped {
    message_id: i64,
    room_id: i64,
},
```

- [ ] **Step 2: Make the WS render layer treat `MessageRegrouped` like `MessageEdited`**

In `server/src/routes/ws.rs`, the `match &e` block inside `handle_socket`'s `send` task currently routes `ChatEvent::MessageEdited { message_id, .. }` through `render_edited_message`. Add `MessageRegrouped` to the same arm:

```rust
ChatEvent::MessageEdited { message_id, .. }
| ChatEvent::MessageRegrouped { message_id, .. } => {
    render_edited_message(&send_state, *message_id, &send_user).await
}
```

In `server/src/views/ws_fragments.rs`, extend the no-op arm in `render_event` so the new variant doesn't break the match:

```rust
ChatEvent::NewMessage { .. }
| ChatEvent::MessageEdited { .. }
| ChatEvent::MessageRegrouped { .. }
| ChatEvent::ReactionAdded { .. }
| ChatEvent::ReactionRemoved { .. }
| ChatEvent::RoomMemberAdded { .. }
| ChatEvent::RoomMemberRemoved { .. }
| ChatEvent::DmRead { .. }
| ChatEvent::UserMuted { .. }
| ChatEvent::UserBanned { .. }
| ChatEvent::UserKicked { .. } => None,
```

- [ ] **Step 3: Wire promote-on-delete into the delete handler**

Edit `server/src/routes/room.rs` `delete_message` (around line 325-349). The current implementation soft-deletes, broadcasts `MessageDeleted`, and returns the deleted-fragment HTML. Add a step between the soft-delete and the `MessageDeleted` broadcast that determines whether the next message needs to be promoted:

Before:

```rust
db::moderation::soft_delete_message(&state.chat, message_id, &user.id).await?;
let event = ChatEvent::MessageDeleted {
    message_id,
    room_id: m.room_id,
};
state.hub.broadcast_to_room(m.room_id, &event);
```

After:

```rust
// Look up the next message in the room BEFORE soft-deleting so the lookup
// can use the simple "id > target.id" predicate without worrying about the
// soft-delete state of the target.
let next = db::chat::next_message_in_room(&state.chat, m.room_id, message_id).await?;

db::moderation::soft_delete_message(&state.chat, message_id, &user.id).await?;

// If the next message was a follow-up of the deleted message (same author,
// within the grouping window), it is now orphaned. Broadcast a regrouping
// event so each connected viewer re-renders that message with the current
// flag (which will now be `false`, because there is no longer a prior
// message in its grouping chain).
if let Some(n) = next.as_ref() {
    let was_follow_up = db::chat::is_follow_up_of(
        Some((m.user_id.as_str(), m.created_at.as_str())),
        (n.user_id.as_str(), n.created_at.as_str()),
    );
    if was_follow_up {
        let regroup = ChatEvent::MessageRegrouped {
            message_id: n.id,
            room_id: m.room_id,
        };
        state.hub.broadcast_to_room(m.room_id, &regroup);
    }
}

let event = ChatEvent::MessageDeleted {
    message_id,
    room_id: m.room_id,
};
state.hub.broadcast_to_room(m.room_id, &event);
```

- [ ] **Step 4: Add a unit test for the promote logic**

Append to `server/tests/message_grouping.rs` (inside the existing `page_grouping` module or a new `delete_promote` module):

```rust
#[tokio::test]
async fn delete_header_marks_next_for_promotion() {
    use lets_chat::db;

    let auth = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let chat = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations/auth").run(&auth).await.unwrap();
    sqlx::migrate!("./migrations/chat").run(&chat).await.unwrap();

    let user_id = db::auth::create_user(&auth, "alice", "x", false).await.unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None).await.unwrap();

    let id1 = db::chat::insert_message(&chat, room_id, &user_id, "first").await.unwrap();
    let _id2 = db::chat::insert_message(&chat, room_id, &user_id, "second").await.unwrap();
    let _id3 = db::chat::insert_message(&chat, room_id, &user_id, "third").await.unwrap();

    let target = db::chat::get_message(&chat, id1).await.unwrap().unwrap();
    let next = db::chat::next_message_in_room(&chat, room_id, id1).await.unwrap().unwrap();

    let was_follow_up = db::chat::is_follow_up_of(
        Some((target.user_id.as_str(), target.created_at.as_str())),
        (next.user_id.as_str(), next.created_at.as_str()),
    );
    assert!(was_follow_up, "next message was a follow-up of the deleted header");

    // After delete, the new prior of `next` is None (id < next.id, all deleted),
    // so the promoted render must produce is_follow_up = false.
    db::moderation::soft_delete_message(&chat, id1, &user_id).await.unwrap();
    let new_prior = db::chat::prior_message_in_room(&chat, room_id, next.id).await.unwrap();
    let promoted_flag = db::chat::is_follow_up_of(
        new_prior.as_ref().map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (next.user_id.as_str(), next.created_at.as_str()),
    );
    assert!(!promoted_flag, "after delete the next message must render as a header");
}

#[tokio::test]
async fn delete_follow_up_does_not_promote_subsequent() {
    use lets_chat::db;

    let auth = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let chat = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations/auth").run(&auth).await.unwrap();
    sqlx::migrate!("./migrations/chat").run(&chat).await.unwrap();

    let user_id = db::auth::create_user(&auth, "alice", "x", false).await.unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None).await.unwrap();

    let id1 = db::chat::insert_message(&chat, room_id, &user_id, "first").await.unwrap();
    let id2 = db::chat::insert_message(&chat, room_id, &user_id, "second").await.unwrap();
    let id3 = db::chat::insert_message(&chat, room_id, &user_id, "third").await.unwrap();

    // Delete id2 (a follow-up). id3 was already a follow-up of id2; after the
    // delete, id3's new prior is id1, which is also from alice within the
    // window, so id3 stays a follow-up. The delete handler still computes
    // `was_follow_up` and broadcasts MessageRegrouped, but the resulting
    // re-render preserves is_follow_up = true. That round-trip is harmless.
    let target = db::chat::get_message(&chat, id2).await.unwrap().unwrap();
    let next = db::chat::next_message_in_room(&chat, room_id, id2).await.unwrap().unwrap();
    assert_eq!(next.id, id3);

    db::moderation::soft_delete_message(&chat, id2, &user_id).await.unwrap();

    let new_prior = db::chat::prior_message_in_room(&chat, room_id, id3).await.unwrap();
    assert_eq!(new_prior.unwrap().id, id1, "id3's new prior is id1");

    let promoted_flag = db::chat::is_follow_up_of(
        Some((target.user_id.as_str(), target.created_at.as_str())),
        (next.user_id.as_str(), next.created_at.as_str()),
    );
    assert!(promoted_flag, "id3 was always a follow-up of its prior");
}
```

- [ ] **Step 5: Run the full test suite**

Run: `./dev/cargo test -p lets-chat-server`

Expected: all tests pass, including the two new promote tests.

- [ ] **Step 6: Manually verify promote-on-delete**

Run: `just dev-web-local`

1. Send three messages in a row from one account.
2. Verify they render as one group (first has header, next two are follow-ups).
3. Delete the first (header) message.
4. The deleted message becomes `[deleted]`; the second message, which was a follow-up, now renders with the username and timestamp header. The third remains a follow-up of the second.

Stop the dev server.

- [ ] **Step 7: Commit**

```bash
git add server/src/ws/events.rs server/src/views/ws_fragments.rs server/src/routes/ws.rs server/src/routes/room.rs server/tests/message_grouping.rs
git commit -m "feat(grouping): promote next message to header when deleting a grouped header"
```

---

## Task 7: Final verification, push, and PR

- [ ] **Step 1: Run all checks**

Run: `just check`

Expected: server check, desktop check, clippy, fmt all pass.

- [ ] **Step 2: Run the full test suite once more**

Run: `./dev/cargo test --workspace`

Expected: all tests pass.

- [ ] **Step 3: Verify the release build still serves the login page**

Run: `just verify`

Expected: builds the release binary, starts it briefly, confirms `GET /login` returns 200 with a form.

- [ ] **Step 4: Push the branch and open a PR**

```bash
git push -u origin feat/message-grouping
```

Then open a PR with title `feat: group consecutive messages from same author` and a body that summarizes the spec, links to the spec file, and lists a manual test plan.

- [ ] **Step 5: Switch back to main**

```bash
git checkout main
```

(User merges the PR. After merge, `git pull` on main per the standard workflow.)
