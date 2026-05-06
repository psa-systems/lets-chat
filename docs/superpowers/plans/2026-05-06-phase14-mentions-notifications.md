# Phase 14 - @mentions + Notifications

## Goal

Let any signed-in user tag another user in a message body with `@username`. The
mentioned user sees an in-app notification surface (title-bar flash, favicon
dot, optional sound, distinct sidebar mention indicator on the room) and a
browser-level notification when the tab is hidden or they are viewing a
different room. Direct messages count as implicit mentions for the same
notification surface; DM "ping" delivery does NOT write to the mentions
table because DM unread state already governs read tracking.

Out of scope (deferred to later phases): Web Push / service worker / VAPID
subscriptions; email digest of missed mentions; per-room mute or notification
preferences; `@here` / `@channel` / `@everyone` (RBAC implications);
mention-edit notification rollback (we re-extract and update the mentions
table when a message is edited, but we never recall a notification that was
already fired).

## Architecture

- **Stack** (current truth): Axum 0.8 + Askama + HTMX. WebSocket payloads are
  pre-rendered HTML fragments tagged with `hx-swap-oob`; never JSON.
- **Mention extraction**. Server-side. On message insert (and on every edit),
  scan the body for `@token` matches against the username pattern, resolve
  each token to a real user via `db::auth::find_user_by_username`, and write
  one row per resolved mention into a new `mentions` table. Self-mentions are
  skipped at insert time (you cannot ping yourself). Room-scope check: the
  mentioned user must be able to see the room (public room in an enclave the
  target belongs to, or explicit `room_members` row for private/DM rooms);
  mentions of users who can't see the room are skipped to avoid leaking room
  existence.
- **`ChatEvent` design - new variant `Mentioned`, NOT extended `NewMessage`.**
  Justification:
  1. Mentions fan out to a *specific user* via the existing
     `Hub::broadcast_to_user(user_id, &event)` channel. Extending
     `NewMessage` would require every WS render path to inspect `mentions:
     Vec<UserId>` and self-route, which duplicates targeting logic the hub
     already has.
  2. The notification surface (title flash, favicon dot, browser
     notification, sidebar `@` indicator) is independent of the message
     foreground/background render. A standalone event keeps `NewMessage`
     focused on "render the message in the open room or bump the unread
     badge" and `Mentioned` focused on "trigger a notification surface".
  3. Edits add or remove mentions. With a separate event we broadcast
     individual `Mentioned`/`MentionCleared` updates only to affected users.
     Folding into `MessageEdited` would require a recipient-side diff and
     a room-wide broadcast for what is really a per-user concern.
- **DM "implicit mention".** When `post_message` lands in a DM room, the
  handler additionally broadcasts `ChatEvent::Mentioned` to the peer with
  `kind = "dm"`. No row is written to the `mentions` table for DMs. The
  client-side notification logic treats DM and room-mention events
  identically.
- **Composer autocomplete - HTMX fragment driven by a tiny JS controller.**
  Pure HTMX with declarative `hx-trigger="keyup"` cannot detect "the cursor
  is currently inside an `@token`". So we use a small (~45 LOC) JS controller
  that:
  1. Watches the textarea for `input`, finds an active `@token` at the
     cursor (via a regex anchored at the cursor position), extracts the
     prefix.
  2. Calls `htmx.ajax('GET', '/users/mentions?room_id=...&q=<prefix>',
     {target: '#lc-mention-popover', swap: 'innerHTML'})`.
  3. Routes ArrowUp/ArrowDown/Enter/Escape between the popover and the
     textarea while the popover has children. Tab works exactly like Enter
     to insert the highlighted suggestion.
  4. On selection, replaces the `@token` substring at the cursor with
     `@<chosen_username> ` and clears the popover.
  The popover content stays in Askama (`partials/mention_popover.html`),
  so chip styling lives in one place.
- **Mention chip rendering.** `MessageView::body_html()` already escapes and
  linkifies. Extend it: pre-attach `mentions: Vec<MentionRef { user_id,
  username }>` to each `MessageView` (bulk-loaded once per page in
  `routes/room.rs::get_room` and `routes/dm.rs::get_dm`). During
  rendering, `@username` substrings that match a known mention render as
  `<a href="/profile/{user_id}" class="text-blue-700 bg-blue-50
  rounded px-1">@username</a>`; everything else takes the existing escape +
  linkify path.
- **Read state.** Mentions are marked read when the viewer's room watermark
  advances past the mention's message id. `mark_room_read` (i.e. the existing
  `db::chat::set_last_read` upsert) gets a sibling call that updates
  `mentions.read_at = datetime('now')` for any rows in this `(room_id,
  mentioned_user_id)` pair with `message_id <= last_read_message_id`. No
  extra round-trip from the client; the existing read-receipts machinery
  drives mention read-state for free.
- **Notification surface ("the bus").** Add an `<div id="lc-notify-bus"
  class="hidden"></div>` slot in the app shell. The WS path renders
  `Mentioned` as a tiny `<div hx-swap-oob="beforeend:#lc-notify-bus"
  data-kind="..." data-room-id="..." data-room-kind="..." data-message-id="..."
  data-author="..." data-snippet="..."></div>` element. A small JS module in
  the shell uses a `MutationObserver` on the bus to:
  - Increment a per-room unread-mention count.
  - Update `document.title` (`(N) lets-chat - #room-name` style).
  - Swap the favicon `<link>` `href` to a dot variant if total unread > 0.
  - Play `/assets/notify.ogg` if the user enabled the sound setting and the
    tab was hidden when the event arrived.
  - Fire a `Notification` (browser API) when the document is hidden or the
    user is currently viewing a different room/DM. Click navigates to the
    room and focuses the window.
  After processing, the bus is emptied (`bus.replaceChildren()`) so memory
  doesn't grow unbounded across a long session.
- **Sidebar mention indicator.** Add `mentions: i64` to `SidebarRoom` (NOT
  `SidebarPeer` - DM mentions are redundant with DM unread). Render a small
  red `@` badge next to the existing blue unread number when `mentions > 0`.
  Swap on the same OOB swap pattern as the existing unread badge: a new
  `<span id="mention-room-{room_id}">` element with a sibling
  `partials/mention_badge.html`.
- **Permission prompt.** `Notification.requestPermission()` is invoked the
  *first* time the bus processes a `Mentioned` event with the user setting
  enabled, never on page load. This avoids the well-known "site asks for
  notification permission as soon as you open it" anti-pattern.
- **Forward-compatible hooks for the deferred per-room mute phase.**
  - The `Mentioned` event carries `room_id`, so a future
    `room_notification_settings(user_id, room_id, mute)` table can be
    consulted in the WS render path without schema or event churn.
  - The bus events carry enough data (room id + room kind + author + snippet)
    to render arbitrary future filters on the client without server changes.

## Tech Stack

- New crates: none. `regex = "1"` is already a transitive dep of
  `linkify`/`scraper`; if the workspace doesn't pin it, add it directly to
  `server/Cargo.toml`. (Confirm in Task 1.)
- New static assets: `server/assets/favicon.svg` (default),
  `server/assets/favicon-dot.svg` (with red dot),
  `server/assets/notify.ogg` (~1-2 KiB synthesised tone).
- Username pattern is currently lenient (no regex check in `db::auth`). The
  parser uses `[A-Za-z0-9_-]{1,32}` to match `@token`. Resolution (NOT the
  parser) decides whether the token is a real user, so the parser stays
  permissive.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `server/migrations/chat/0014_mentions.sql` | `mentions` table + index for unread-mentions query. |
| Add | `server/migrations/auth/0007_notification_settings.sql` | `notify_browser_enabled INT NOT NULL DEFAULT 1` + `notify_sound_enabled INT NOT NULL DEFAULT 0` columns on `users`. |
| Add | `server/src/db/mentions.rs` | `parse_mention_tokens`, `insert_mentions_for_message`, `delete_mentions_for_message`, `mentions_for_messages` (bulk loader), `mark_mentions_read_for_room`, `count_unread_mentions_per_room`. |
| Edit | `server/src/db/mod.rs` | `pub mod mentions;`. |
| Edit | `server/src/db/auth.rs` | Add `notify_browser_enabled` / `notify_sound_enabled` to every `UserRecord` SELECT and the `User` mapping; add `set_notification_prefs` setter. |
| Edit | `server/src/models/user.rs` | Add the two `notify_*` boolean fields to `UserRecord` and `User`. |
| Add | `server/src/views/mentions.rs` | `MentionRef { user_id, username }` view type + autocomplete-popover Askama template struct. |
| Edit | `server/src/views/room.rs` | Add `mentions: Vec<MentionRef>` to `MessageView`; update `body_html()` to render chips. Add `mentions: i64` to `SidebarRoom` (in `views/layout.rs` actually - see below). |
| Edit | `server/src/views/layout.rs` | Add `mentions: i64` to `SidebarRoom`. |
| Edit | `server/src/views/ws_fragments.rs` | New `MentionedFragment` and `MentionBadgeFragment` Askama structs; extend `render_event` to skip `Mentioned`/`MentionCleared` (rendered per-recipient in `routes/ws.rs`). |
| Add | `server/src/routes/mentions.rs` | `GET /users/mentions?room_id=&q=` autocomplete endpoint. |
| Edit | `server/src/routes/mod.rs` | `mod mentions;` + `.route("/users/mentions", get(mentions::get_autocomplete))`. Add `lc-notify-bus` slot rendering helper if not template-only. |
| Edit | `server/src/routes/room.rs` | `post_message` extracts mentions, inserts rows, broadcasts `Mentioned` per resolved target. Bulk-load `mentions` per page in `get_room`. `patch_message` re-extracts and reconciles. |
| Edit | `server/src/routes/dm.rs` | Bulk-load mentions per page in `get_dm` (no inserts; DMs don't write mention rows). DM peer gets a `Mentioned { kind: "dm" }` event from the post path. |
| Edit | `server/src/routes/ws.rs` | Add a `render_mentioned(...)` arm. Mark mentions read inside `render_new_message_or_bump` when the viewer is auto-marked-read on the foreground path. |
| Edit | `server/src/routes/settings.rs` | `SettingsForm` gains `notify_browser_enabled` + `notify_sound_enabled`; `post_settings` writes them. |
| Edit | `server/src/ws/events.rs` | Add `ChatEvent::Mentioned` and `ChatEvent::MentionCleared`. |
| Add | `server/templates/partials/mention_popover.html` | Autocomplete dropdown. Empty render when no matches. |
| Add | `server/templates/partials/mention_badge.html` | Sidebar `@N` indicator. |
| Add | `server/templates/ws/mentioned.html` | Tiny OOB `<div data-...>` appended to `#lc-notify-bus`. |
| Add | `server/templates/ws/mention_cleared.html` | OOB element bumping `#lc-notify-bus` with a "decrement room R" instruction. |
| Edit | `server/templates/room/composer.html` | `<div id="lc-mention-popover">` placeholder + ~45 lines of inline JS for tokenizer + keyboard nav + selection insertion. |
| Edit | `server/templates/partials/sidebar.html` | Include `partials/mention_badge.html` next to the existing `unread_badge.html` for room rows. |
| Edit | `server/templates/layout.html` | `<div id="lc-notify-bus" class="hidden"></div>` slot + ~60 lines of inline JS that runs the MutationObserver, title flash, favicon swap, sound, browser notification, and click→focus. |
| Edit | `server/templates/settings/page.html` | Two new checkboxes under "Preferences" for browser notifications + sound. |
| Add | `server/assets/favicon.svg` | Default favicon. |
| Add | `server/assets/favicon-dot.svg` | Favicon with red dot. |
| Add | `server/assets/notify.ogg` | Short notification sound. |
| Add | `server/tests/db_mentions.rs` | DB tests: parse, insert, scope-respect, read-watermark advance, edit reconciliation. |
| Add | `server/tests/routes_mentions.rs` | HTTP tests: autocomplete returns enclave/room-scoped users; sending `@bob` inserts a row; editing to remove `@bob` deletes the row; mention badge fragment renders. |
| Edit | `server/tests/db_read_receipts.rs` and every other `tests/*.rs` that builds a chat pool | Add migration `0014` to the `setup_*_pool` for-loop. |

## Tasks

### Task 1 - Schema, deps, view-model fields

- [ ] Confirm next migration numbers: `ls server/migrations/chat/` -> next is
      **`0014`**; `ls server/migrations/auth/` -> next is **`0007`**.
- [ ] If `regex` is not already a direct dep, add to `server/Cargo.toml`:

```toml
regex = "1"
```

- [ ] Create `server/migrations/chat/0014_mentions.sql`:

```sql
CREATE TABLE IF NOT EXISTS mentions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id          INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    room_id             INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    mentioned_user_id   TEXT NOT NULL,
    author_user_id      TEXT NOT NULL,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    read_at             TEXT,
    UNIQUE (message_id, mentioned_user_id)
);

CREATE INDEX IF NOT EXISTS idx_mentions_unread
    ON mentions (mentioned_user_id, read_at);

CREATE INDEX IF NOT EXISTS idx_mentions_room_user
    ON mentions (room_id, mentioned_user_id);

CREATE INDEX IF NOT EXISTS idx_mentions_message
    ON mentions (message_id);
```

- [ ] Create `server/migrations/auth/0007_notification_settings.sql`:

```sql
ALTER TABLE users ADD COLUMN notify_browser_enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN notify_sound_enabled   INTEGER NOT NULL DEFAULT 0;
```

  Default `notify_browser_enabled = 1` because a user can still decline the
  permission prompt; the column is the *user setting*, not the OS-level
  permission. `notify_sound_enabled` defaults off to avoid surprise audio.
- [ ] Edit `server/src/models/user.rs`. Append to `UserRecord` and `User`:

```rust
pub notify_browser_enabled: bool,
pub notify_sound_enabled: bool,
```

  Update the `From<UserRecord> for User` impl to forward both fields.
- [ ] Edit every `SELECT id, username, ...` in `server/src/db/auth.rs` to
      include `notify_browser_enabled` and `notify_sound_enabled`. The
      offending functions are `find_user_by_username`, `find_user_by_id`,
      `list_users`, `search_users`, plus any `row_to_user_record` helper
      that materializes a `UserRecord`. Sample updated SELECT:

```sql
SELECT id, username, display_name, password_hash, role,
       is_banned, ban_reason, banned_until,
       is_muted, muted_until, mute_reason,
       created_at, updated_at, read_receipts_enabled,
       bio, avatar_ext, status, custom_status, last_active_at, is_profile_public,
       notify_browser_enabled, notify_sound_enabled
  FROM users ...
```

  And `row_to_user_record` gains:

```rust
notify_browser_enabled: row.get::<i64, _>("notify_browser_enabled") != 0,
notify_sound_enabled:   row.get::<i64, _>("notify_sound_enabled")   != 0,
```

- [ ] Add to `server/src/db/auth.rs`:

```rust
pub async fn set_notification_prefs(
    pool: &SqlitePool,
    user_id: &str,
    browser: bool,
    sound: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users \
            SET notify_browser_enabled = ?, \
                notify_sound_enabled   = ?, \
                updated_at             = datetime('now') \
          WHERE id = ?",
    )
    .bind(browser as i32)
    .bind(sound as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] Add `mentions: i64` (default 0) to `SidebarRoom` in
      `server/src/views/layout.rs`. Update every `SidebarRoom { ... }` literal
      (sidebar loader paths in `routes/mod.rs::load_sidebar` and
      `load_chrome`) to set `mentions: 0` for now; Task 6 wires the real
      count.
- [ ] Update `server/tests/db_read_receipts.rs`, `server/tests/db_dm.rs`,
      `server/tests/db_private_rooms.rs`, `server/tests/message_editing.rs`,
      `server/tests/db_moderation.rs`, `server/tests/db_auth.rs`,
      `server/tests/rbac.rs`, `server/tests/db_invite.rs`,
      `server/tests/db_settings.rs`, `server/tests/db_reactions.rs`,
      `server/tests/db_search.rs`, `server/tests/db_status.rs`,
      `server/tests/db_uploads.rs`, `server/tests/last_visited.rs`,
      `server/tests/message_grouping.rs`, `server/tests/db_enclave.rs`,
      `server/tests/migration_enclaves.rs`, `server/tests/perms.rs`,
      `server/tests/routes_enclave.rs`, and `server/tests/routes_uploads.rs`
      to register `0014` and `0007`. Pattern (chat side):

```rust
include_str!("../migrations/chat/0014_mentions.sql"),
```

      And auth side:

```rust
include_str!("../migrations/auth/0007_notification_settings.sql"),
```

      Each test file has a single `setup_*_pool()` helper; only one edit per
      file.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git checkout -b feat/mentions-notifications`
- [ ] `git add server/Cargo.toml server/migrations/chat/0014_mentions.sql server/migrations/auth/0007_notification_settings.sql server/src/db/auth.rs server/src/models/user.rs server/src/views/layout.rs server/src/routes/ server/tests/`

### Task 2 - Mention parser + DB layer

- [ ] Create `server/src/db/mentions.rs`:

```rust
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};

/// Username characters we accept inside an `@token`. The auth layer is
/// permissive about what it accepts at registration time, so this is a
/// best-effort token shape; final resolution is by exact lookup against
/// `users.username`.
const TOKEN_PATTERN: &str = r"@([A-Za-z0-9_-]{1,32})";

pub fn parse_mention_tokens(body: &str) -> Vec<String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(TOKEN_PATTERN).expect("valid regex"));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cap in re.captures_iter(body) {
        let token = cap.get(1).unwrap().as_str().to_string();
        if seen.insert(token.clone()) {
            out.push(token);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct MentionRef {
    pub user_id: String,
    pub username: String,
}

/// Replace the mention set for `message_id` with `targets`. Removes rows for
/// users not in `targets`, inserts rows for users not previously mentioned,
/// preserves `read_at` for users mentioned both before and after.
///
/// Returns `(added, removed)` so the caller can fan out per-user
/// `Mentioned` / `MentionCleared` events.
pub async fn reconcile_mentions(
    pool: &SqlitePool,
    message_id: i64,
    room_id: i64,
    author_user_id: &str,
    targets: &[MentionRef],
) -> Result<(Vec<MentionRef>, Vec<MentionRef>), sqlx::Error> {
    // Load existing.
    let existing_rows = sqlx::query(
        "SELECT mentioned_user_id FROM mentions WHERE message_id = ?",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;
    let existing: HashSet<String> = existing_rows
        .into_iter()
        .map(|r| r.get::<String, _>("mentioned_user_id"))
        .collect();
    let next: HashMap<String, MentionRef> = targets
        .iter()
        .map(|m| (m.user_id.clone(), m.clone()))
        .collect();

    let added: Vec<MentionRef> = next
        .iter()
        .filter(|(id, _)| !existing.contains(*id))
        .map(|(_, m)| m.clone())
        .collect();
    let removed: Vec<MentionRef> = existing
        .iter()
        .filter(|id| !next.contains_key(*id))
        // We don't have the username for removed targets without another
        // lookup; the caller only uses `removed` to emit MentionCleared,
        // which carries user_id only.
        .map(|id| MentionRef {
            user_id: id.clone(),
            username: String::new(),
        })
        .collect();

    for m in &added {
        sqlx::query(
            "INSERT OR IGNORE INTO mentions \
                 (message_id, room_id, mentioned_user_id, author_user_id) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(room_id)
        .bind(&m.user_id)
        .bind(author_user_id)
        .execute(pool)
        .await?;
    }
    for m in &removed {
        sqlx::query("DELETE FROM mentions WHERE message_id = ? AND mentioned_user_id = ?")
            .bind(message_id)
            .bind(&m.user_id)
            .execute(pool)
            .await?;
    }
    Ok((added, removed))
}

/// Bulk-load mentions for a page of messages. Used by `routes/room.rs`
/// and `routes/dm.rs` to attach mention chip metadata to each `MessageView`
/// without N+1 queries.
pub async fn mentions_for_messages(
    pool: &SqlitePool,
    auth_pool: &SqlitePool,
    message_ids: &[i64],
) -> Result<HashMap<i64, Vec<MentionRef>>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat("?").take(message_ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT message_id, mentioned_user_id FROM mentions WHERE message_id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;

    // Resolve each user_id to a username from the auth pool. Cache by id.
    let mut cache: HashMap<String, String> = HashMap::new();
    let mut by_message: HashMap<i64, Vec<MentionRef>> = HashMap::new();
    for r in rows {
        let mid: i64 = r.get("message_id");
        let uid: String = r.get("mentioned_user_id");
        let username = if let Some(u) = cache.get(&uid) {
            u.clone()
        } else {
            let user = crate::db::auth::find_user_by_id(auth_pool, &uid).await?;
            let name = user.map(|u| u.username).unwrap_or_else(|| uid.clone());
            cache.insert(uid.clone(), name.clone());
            name
        };
        by_message.entry(mid).or_default().push(MentionRef {
            user_id: uid,
            username,
        });
    }
    Ok(by_message)
}

/// Mark every mention of `user_id` in `room_id` with `message_id <= watermark`
/// as read. Called from the same path as `set_last_read`.
pub async fn mark_mentions_read_for_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    watermark: i64,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE mentions \
            SET read_at = datetime('now') \
          WHERE mentioned_user_id = ? \
            AND room_id = ? \
            AND message_id <= ? \
            AND read_at IS NULL",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(watermark)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Per-room unread mention counts for the sidebar. Returns rows where
/// count > 0.
pub async fn count_unread_mentions_per_room(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT room_id, COUNT(*) AS n \
           FROM mentions \
          WHERE mentioned_user_id = ? AND read_at IS NULL \
          GROUP BY room_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<i64, _>("room_id"), r.get::<i64, _>("n")))
        .collect())
}
```

- [ ] Add `pub mod mentions;` to `server/src/db/mod.rs`.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/db/mentions.rs server/src/db/mod.rs`

### Task 3 - ChatEvent variants

- [ ] Edit `server/src/ws/events.rs`. Add inside `ChatEvent`:

```rust
/// A user was @-mentioned in a room message, or a DM was sent to them.
/// Routed via `Hub::broadcast_to_user(mentioned_user_id, ...)`.
Mentioned {
    /// "mention" for a real `@username` ping in a room; "dm" for an
    /// implicit DM ping. The client uses this to label the notification.
    kind: String,
    room_id: i64,
    /// "public" | "private" | "dm" - lets the client format the
    /// notification title without an extra DB lookup.
    room_type: String,
    /// Display label for the room (e.g. "#general") or DM peer name.
    room_label: String,
    message_id: i64,
    mentioned_user_id: String,
    author_label: String,
    /// First ~140 chars of the body, plain-text. Mention chips and links
    /// are stripped to keep the notification readable.
    snippet: String,
    /// "/room/{id}" or "/dm/{peer_id}" - target path for the click handler.
    target_path: String,
},
/// A previously-fired mention is no longer current (the message was
/// edited to remove the @-token, or the message was deleted). The client
/// decrements its in-memory unread-mention count for that room.
MentionCleared {
    room_id: i64,
    mentioned_user_id: String,
    message_id: i64,
},
```

- [ ] In `server/src/views/ws_fragments.rs::render_event`, extend the
      not-rendered match arms list to include `Mentioned`/`MentionCleared`
      (they are rendered per-recipient in `routes/ws.rs`).
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/ws/events.rs server/src/views/ws_fragments.rs`

### Task 4 - Autocomplete endpoint

- [ ] Create `server/src/routes/mentions.rs`:

```rust
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::mentions::{MentionPopoverFragment, MentionSuggestion};
use crate::views::{html, Html};

const MAX: usize = 8;

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    pub room_id: i64,
    #[serde(default)]
    pub q: String,
}

/// GET /users/mentions?room_id=&q=
///
/// Returns a small `<ul>` of users the caller is allowed to @ in `room_id`.
/// Empty/whitespace `q` returns a recently-active subset (top 5 members of
/// the room). Always returns 200 with an HTML body so the composer's
/// `htmx.ajax(...)` can swap directly into the popover slot.
pub async fn get_autocomplete(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Query(AutocompleteQuery { room_id, q }): Query<AutocompleteQuery>,
) -> Result<Html, AppError> {
    let trimmed = q.trim();

    // Access guard: viewer must be able to read the room to mention into it.
    let is_admin = viewer.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &viewer.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // Determine the mention pool. For private rooms and DMs, that's the
    // explicit `room_members`. For public rooms, that's the rooms's enclave
    // members. (DMs in practice need no autocomplete - there's only one
    // peer - but we serve consistent behaviour.)
    let candidate_ids = candidate_ids(&state, room_id).await?;

    let viewer_id = viewer.id.clone();
    let mut results: Vec<MentionSuggestion> = Vec::with_capacity(MAX);
    for id in candidate_ids {
        if id == viewer_id {
            continue;
        }
        if results.len() >= MAX {
            break;
        }
        let Some(rec) = db::auth::find_user_by_id(&state.auth, &id).await? else { continue };
        if rec.is_banned {
            continue;
        }
        if !trimmed.is_empty() {
            let q_lower = trimmed.to_ascii_lowercase();
            let uname = rec.username.to_ascii_lowercase();
            let dname = rec
                .display_name
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if !uname.contains(&q_lower) && !dname.contains(&q_lower) {
                continue;
            }
        }
        results.push(MentionSuggestion {
            user_id: rec.id,
            username: rec.username,
            display_name: rec.display_name,
            avatar_ext: rec.avatar_ext,
        });
    }

    let frag = MentionPopoverFragment { results: &results };
    html(&frag)
}

async fn candidate_ids(
    state: &AppState,
    room_id: i64,
) -> Result<Vec<String>, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "private" || room.room_type == "dm" {
        return Ok(db::chat::list_room_member_ids(&state.chat, room_id).await?);
    }
    // Public room: scope to enclave members. If the room is unenclave'd
    // (legacy rows), fall back to all users.
    if let Some(enclave_id) = room.enclave_id {
        return Ok(db::enclave::list_member_ids(&state.chat, enclave_id).await?);
    }
    Ok(db::auth::list_user_ids(&state.auth).await?)
}
```

  Helpers `db::enclave::list_member_ids` and `db::auth::list_user_ids` may
  not exist yet as 1-line wrappers. Check; add them as 5-line wrappers if
  missing.
- [ ] Add `server/src/views/mentions.rs`:

```rust
use askama::Template;

#[derive(Clone)]
pub struct MentionSuggestion {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
}

#[derive(Template)]
#[template(path = "partials/mention_popover.html")]
pub struct MentionPopoverFragment<'a> {
    pub results: &'a [MentionSuggestion],
}

#[derive(Clone)]
pub struct MentionRef {
    pub user_id: String,
    pub username: String,
}
```

- [ ] Re-export `MentionRef` from `server/src/views/mod.rs` so
      `views::room::MessageView` can reference it without a deep path.
- [ ] Add `mod mentions;` to `server/src/routes/mod.rs` and register the
      route in `build_router`:

```rust
.route("/users/mentions", get(mentions::get_autocomplete))
```

- [ ] Create `server/templates/partials/mention_popover.html`:

```html
{% if results.is_empty() %}
<ul id="lc-mention-list" class="hidden"></ul>
{% else %}
<ul id="lc-mention-list" class="absolute z-30 bottom-full mb-1 max-h-64 w-64 overflow-y-auto rounded border border-slate-200 bg-white shadow-lg" role="listbox">
  {% for s in results %}
  <li role="option">
    <button type="button"
      class="w-full text-left px-2 py-1 hover:bg-slate-100 aria-selected:bg-blue-100 flex items-center gap-2"
      data-username="{{ s.username }}">
      {% let avatar_user_id = s.user_id.clone() %}
      {% let avatar_username = s.username.clone() %}
      {% let avatar_ext = s.avatar_ext.clone() %}
      {% let avatar_status = "active".to_string() %}
      {% let avatar_custom_status = None::<String> %}
      {% let avatar_size = "h-5 w-5 text-xs" %}
      {% include "partials/avatar.html" %}
      <span class="font-medium">{{ s.username }}</span>
      {% if let Some(dn) = s.display_name.as_ref() %}
      <span class="text-xs text-slate-500 truncate">{{ dn }}</span>
      {% endif %}
    </button>
  </li>
  {% endfor %}
</ul>
{% endif %}
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/mentions.rs server/src/views/mentions.rs server/src/views/mod.rs server/src/routes/mod.rs server/templates/partials/mention_popover.html`

### Task 5 - Composer JS controller

- [ ] Edit `server/templates/room/composer.html`. Inside the existing
      `<form id="composer">`, ABOVE the `<div class="flex items-end gap-2">`,
      add an empty popover slot:

```html
<div id="lc-mention-popover" class="relative"></div>
```

- [ ] Append the following script just before the existing
      file/drag/drop `<script>` block. Hard cap: 50 lines (this fits in 45).

```html
<script>
(function(){
  var ta = document.querySelector('#composer textarea[name=body]');
  var slot = document.getElementById('lc-mention-popover');
  if (!ta || !slot) return;
  var roomId = {{ room.id }};
  var TOKEN = /(^|\s)@([A-Za-z0-9_-]{0,32})$/;
  var current = null; // { start, end, prefix }

  function activeToken() {
    var pos = ta.selectionStart;
    var before = ta.value.slice(0, pos);
    var m = before.match(TOKEN);
    if (!m) return null;
    var prefix = m[2];
    var start = pos - prefix.length - 1;
    return { start: start, end: pos, prefix: prefix };
  }
  function close() { current = null; slot.replaceChildren(); }
  function refresh() {
    current = activeToken();
    if (!current) return close();
    var url = '/users/mentions?room_id=' + roomId + '&q=' + encodeURIComponent(current.prefix);
    htmx.ajax('GET', url, { target: '#lc-mention-popover', swap: 'innerHTML' });
  }
  function items() { return slot.querySelectorAll('button[data-username]'); }
  function selected() { return slot.querySelector('button[aria-selected="true"]'); }
  function select(btn) {
    items().forEach(function(b){ b.setAttribute('aria-selected', b === btn ? 'true' : 'false'); });
    if (btn && btn.scrollIntoView) btn.scrollIntoView({ block: 'nearest' });
  }
  function insert(btn) {
    if (!current || !btn) return close();
    var u = btn.getAttribute('data-username');
    var v = ta.value;
    ta.value = v.slice(0, current.start) + '@' + u + ' ' + v.slice(current.end);
    var pos = current.start + u.length + 2;
    ta.selectionStart = ta.selectionEnd = pos;
    close();
    ta.dispatchEvent(new Event('input'));
  }
  ta.addEventListener('input', refresh);
  ta.addEventListener('blur', function(){ setTimeout(close, 100); });
  ta.addEventListener('keydown', function(e){
    var list = items();
    if (!current || list.length === 0) return;
    var sel = selected() || list[0];
    var idx = Array.prototype.indexOf.call(list, sel);
    if (e.key === 'ArrowDown')      { e.preventDefault(); select(list[Math.min(idx + 1, list.length - 1)]); }
    else if (e.key === 'ArrowUp')   { e.preventDefault(); select(list[Math.max(idx - 1, 0)]); }
    else if (e.key === 'Enter' || e.key === 'Tab') { e.preventDefault(); insert(sel); }
    else if (e.key === 'Escape')    { e.preventDefault(); close(); }
  });
  document.body.addEventListener('htmx:afterSwap', function(e){
    if (e.target && e.target.id === 'lc-mention-popover') {
      var first = slot.querySelector('button[data-username]');
      if (first) first.setAttribute('aria-selected', 'true');
      slot.addEventListener('mousedown', function(ev){
        var b = ev.target.closest('button[data-username]');
        if (b) { ev.preventDefault(); insert(b); }
      }, { once: true });
    }
  });
})();
</script>
```

  Notes:
  - The existing textarea `onkeydown` handler in `composer.html` already
    uses Enter to send. The new keydown listener is registered AFTER the
    existing one (later in DOM order, but both bubble at the same phase).
    Both listeners call `preventDefault()` on Enter when they handle it.
    The mention listener early-returns when no popover is open, so
    Enter-to-send keeps working when there's no `@token` active. Verify
    in the manual smoke step.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/templates/room/composer.html`

### Task 6 - Mention extraction + chip rendering on send + page render

- [ ] Edit `server/src/views/room.rs`. Add `pub mentions:
      Vec<crate::views::mentions::MentionRef>` to `MessageView`. Update
      every `MessageView { ... }` literal to set `mentions: Vec::new()`
      where the bulk-loader doesn't yet fill it. Update `body_html()`:

```rust
pub fn body_html(&self) -> String {
    render_body(&self.body, &self.mentions)
}
```

  Add a free function `render_body(body: &str, mentions: &[MentionRef]) ->
  String` that:
  1. Builds a HashSet<String> of mentioned usernames (lowercased) for
     O(1) match against tokens during scan.
  2. Walks the body once, emitting:
     - HTML-escaped runs of plain text up to each detected `@token`
       (regex-driven, same `TOKEN_PATTERN` as in `db::mentions`).
     - For each token whose username matches a known mention, emit
       `<a href="/profile/{user_id}" class="text-blue-700 bg-blue-50
       rounded px-1">@{username}</a>`. The `user_id` comes from looking
       the username up in `mentions` (case-insensitive).
     - For tokens that don't match a known mention, emit the escaped
       literal `@token` so unresolved tokens stay readable text.
  3. After the mention pass, runs the existing linkify on the *escaped*
     output. Linkify on the chip-output is safe because the chip's `href`
     uses `/profile/...` which linkify won't double-wrap (it only matches
     URLs, not relative paths in href attributes parsed as text - and
     linkify operates on text, not HTML, so the chip's anchor text is
     `@username` which contains no URL scheme).
  Caveat: order matters. Mention substitution must come BEFORE linkify, or
  a token like `@foo.com` could be interpreted as a URL. Confirm with a
  test in Task 11.
- [ ] Edit `server/src/routes/room.rs::get_room`. After the
      `attachments_for_messages` bulk-load, add:

```rust
let mut mentions_by_message =
    db::mentions::mentions_for_messages(&state.chat, &state.auth, &message_ids).await?;
```

  In the `for m in raw_messages` loop, set:

```rust
mentions: mentions_by_message.remove(&m.id).unwrap_or_default(),
```

- [ ] Same change in `server/src/routes/dm.rs::get_dm`.
- [ ] Edit `server/src/routes/room.rs::post_message`. After
      `db::chat::insert_message`, before `broadcast_room_message`:

```rust
// Mention extraction. Skipped for DM rooms - DMs are implicit pings, no
// mention rows.
let mention_targets: Vec<crate::db::mentions::MentionRef> = if room.room_type != "dm" {
    let tokens = crate::db::mentions::parse_mention_tokens(body);
    let candidates = candidate_ids_for_room(&state, &room).await?;
    let candidate_set: std::collections::HashSet<&str> =
        candidates.iter().map(String::as_str).collect();
    let mut resolved = Vec::new();
    for token in tokens {
        if let Some(rec) = db::auth::find_user_by_username(&state.auth, &token).await? {
            if rec.id == user.id { continue; }                    // no self-pings
            if !candidate_set.contains(rec.id.as_str()) { continue; } // out of room scope
            resolved.push(crate::db::mentions::MentionRef {
                user_id: rec.id,
                username: rec.username,
            });
        }
    }
    resolved
} else {
    Vec::new()
};

let (added, _removed) = if !mention_targets.is_empty() {
    db::mentions::reconcile_mentions(
        &state.chat, new_id, room.id, &user.id, &mention_targets,
    ).await?
} else {
    (Vec::new(), Vec::new())
};
```

  Add `candidate_ids_for_room` as a private helper in the same file
  (mirroring the autocomplete helper in `routes/mentions.rs`).
- [ ] After `super::broadcast_room_message(...)`, fan out the per-target
      `Mentioned` events:

```rust
let snippet = build_snippet(body); // ~140 char plain-text strip
for target in &added {
    let label = author_display_label(&user); // displays display_name if set
    let event = ChatEvent::Mentioned {
        kind: "mention".into(),
        room_id: room.id,
        room_type: room.room_type.clone(),
        room_label: format!("#{}", room.name),
        message_id: new_id,
        mentioned_user_id: target.user_id.clone(),
        author_label: label,
        snippet: snippet.clone(),
        target_path: format!("/room/{}", room.id),
    };
    state.hub.broadcast_to_user(&target.user_id, &event);
}

// Implicit DM ping. Routed to the peer regardless of subscription state.
if room.room_type == "dm" {
    if let Some(peer_id) = db::chat::list_room_member_ids(&state.chat, room.id)
        .await?
        .into_iter()
        .find(|id| id != &user.id)
    {
        let event = ChatEvent::Mentioned {
            kind: "dm".into(),
            room_id: room.id,
            room_type: "dm".into(),
            room_label: author_display_label(&user),
            message_id: new_id,
            mentioned_user_id: peer_id.clone(),
            author_label: author_display_label(&user),
            snippet: build_snippet(body),
            target_path: format!("/dm/{}", user.id),
        };
        state.hub.broadcast_to_user(&peer_id, &event);
    }
}
```

  `build_snippet` (private helper in `routes/room.rs`): trim, collapse
  whitespace, strip mention tokens to plain `@username` (no chip syntax),
  truncate at 140 chars, append "..." if truncated.
- [ ] Edit `server/src/routes/room.rs::patch_message`. After the
      `update_message_body` call:

```rust
// Re-extract mentions for the edited body. Reconcile against existing rows.
let mention_targets = if m.room_id_room_type_is_dm() {
    Vec::new()
} else {
    let body = body.to_string();
    let tokens = crate::db::mentions::parse_mention_tokens(&body);
    let room = db::chat::get_room(&state.chat, m.room_id).await?
        .ok_or(AppError::NotFound)?;
    let candidates = candidate_ids_for_room(&state, &room).await?;
    let candidate_set: std::collections::HashSet<&str> =
        candidates.iter().map(String::as_str).collect();
    let mut resolved = Vec::new();
    for token in tokens {
        if let Some(rec) = db::auth::find_user_by_username(&state.auth, &token).await? {
            if rec.id == user.id { continue; }
            if !candidate_set.contains(rec.id.as_str()) { continue; }
            resolved.push(crate::db::mentions::MentionRef {
                user_id: rec.id, username: rec.username,
            });
        }
    }
    resolved
};
let (added, removed) = db::mentions::reconcile_mentions(
    &state.chat, message_id, m.room_id, &user.id, &mention_targets,
).await?;
// Fan out new mentions to newly added users.
for t in &added { /* same Mentioned event as in post_message */ }
// Tell newly-removed users to decrement their unread count.
for t in &removed {
    let event = ChatEvent::MentionCleared {
        room_id: m.room_id,
        mentioned_user_id: t.user_id.clone(),
        message_id,
    };
    state.hub.broadcast_to_user(&t.user_id, &event);
}
```

  (`m.room_id_room_type_is_dm()` is shorthand; if no helper exists, fetch
  the room and check `room.room_type == "dm"`.)
- [ ] Soft delete: in the existing `delete_message` handler, after the
      moderation soft-delete, broadcast `MentionCleared` for every row in
      `mentions WHERE message_id = ?` so badges decrement, then DELETE
      those rows. Hard-delete cascade already drops them on full row
      removal but soft-delete leaves the row, so we do the explicit
      cleanup.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/views/room.rs server/src/routes/room.rs server/src/routes/dm.rs`

### Task 7 - WS render path: per-recipient `Mentioned` + auto-mark-read

- [ ] In `server/src/routes/ws.rs`, in the `match &e` block inside
      `send`, add:

```rust
ChatEvent::Mentioned { mentioned_user_id, .. } if mentioned_user_id == &send_user.id => {
    render_mentioned(&e)
}
ChatEvent::MentionCleared { mentioned_user_id, .. } if mentioned_user_id == &send_user.id => {
    render_mention_cleared(&e)
}
```

  Note the guard: `broadcast_to_user` already filters by user, but the same
  receive path may see other users' fan-out if a future change widens the
  channel. The guard is a belt-and-braces self-check.
- [ ] Add the two render helpers (analogous to the existing
      `render_dm_read`):

```rust
fn render_mentioned(event: &ChatEvent) -> Option<String> {
    let ChatEvent::Mentioned { kind, room_id, room_type, room_label,
        message_id, author_label, snippet, target_path, .. } = event else { return None };
    MentionedFragment {
        kind, room_id: *room_id, room_type, room_label,
        message_id: *message_id, author_label, snippet, target_path,
    }.render().ok()
}
fn render_mention_cleared(event: &ChatEvent) -> Option<String> {
    let ChatEvent::MentionCleared { room_id, message_id, .. } = event else { return None };
    MentionClearedFragment { room_id: *room_id, message_id: *message_id }.render().ok()
}
```

- [ ] Add the two fragment types to
      `server/src/views/ws_fragments.rs`:

```rust
#[derive(Template)]
#[template(path = "ws/mentioned.html")]
pub struct MentionedFragment<'a> {
    pub kind: &'a str,
    pub room_id: i64,
    pub room_type: &'a str,
    pub room_label: &'a str,
    pub message_id: i64,
    pub author_label: &'a str,
    pub snippet: &'a str,
    pub target_path: &'a str,
}

#[derive(Template)]
#[template(path = "ws/mention_cleared.html")]
pub struct MentionClearedFragment {
    pub room_id: i64,
    pub message_id: i64,
}
```

- [ ] Create `server/templates/ws/mentioned.html`:

```html
<div hx-swap-oob="beforeend:#lc-notify-bus"
     data-event="mentioned"
     data-kind="{{ kind }}"
     data-room-id="{{ room_id }}"
     data-room-type="{{ room_type }}"
     data-room-label="{{ room_label }}"
     data-message-id="{{ message_id }}"
     data-author="{{ author_label }}"
     data-snippet="{{ snippet }}"
     data-target="{{ target_path }}"></div>
```

- [ ] Create `server/templates/ws/mention_cleared.html`:

```html
<div hx-swap-oob="beforeend:#lc-notify-bus"
     data-event="mention_cleared"
     data-room-id="{{ room_id }}"
     data-message-id="{{ message_id }}"></div>
```

- [ ] In `routes/ws.rs::render_new_message_or_bump`, when the foreground
      branch fires the `set_last_read` upsert, also call:

```rust
db::mentions::mark_mentions_read_for_room(
    &state.chat, &viewer.id, message.room_id, message.id,
).await.ok();
```

      Same call goes inside `routes/room.rs::get_room` and
      `routes/dm.rs::get_dm` right next to the existing
      `set_last_read` invocation, so the page-render path also clears
      mention rows past the watermark. (One UPDATE; cheap.)
- [ ] Render a `MentionCleared`-equivalent for the viewer's other tabs by
      broadcasting it in the same place where `set_last_read` broadcasts
      `DmRead` today: when the user opens the room from one tab, all of
      their tabs need to drop the badge. Re-use the existing `DmRead`
      broadcast - the client interprets DmRead as "rebuild this room's
      badges" and that already covers the mention badge once Task 8 lands
      it.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/ws.rs server/src/views/ws_fragments.rs server/templates/ws/mentioned.html server/templates/ws/mention_cleared.html`

### Task 8 - Sidebar mention badge wiring

- [ ] Edit `server/src/routes/mod.rs::load_sidebar` (or whichever helper
      builds `Vec<SidebarRoom>`). After computing per-room unread counts,
      compute per-room mention counts:

```rust
let mention_counts: HashMap<i64, i64> =
    db::mentions::count_unread_mentions_per_room(&state.chat, &user.id)
        .await?
        .into_iter()
        .collect();
```

  Set each `SidebarRoom { ..., mentions: *mention_counts.get(&id).unwrap_or(&0) }`.
- [ ] Create `server/templates/partials/mention_badge.html`:

```html
{% if mentions > 0 %}
<span id="mention-room-{{ id }}" class="ml-1 text-[10px] font-bold uppercase bg-red-600 text-white rounded px-1.5">@{{ mentions }}</span>
{% else %}
<span id="mention-room-{{ id }}"></span>
{% endif %}
```

- [ ] Edit `server/templates/partials/sidebar.html`. In the rooms loop,
      after the existing `unread_badge.html` include, add:

```html
{% let mentions = room.mentions %}
{% include "partials/mention_badge.html" %}
```

  (The `id` and `kind` lets are already set above.)
- [ ] Add a `MentionBadgeFragment` to `views/ws_fragments.rs` so the WS
      layer can OOB-update a single badge when a `Mentioned` event's
      MutationObserver-derived increment happens server-side. Actually:
      the per-event server-side update would be redundant given the
      client-side bus already increments locally. **Decision:** keep
      mention-badge updates fully client-side via the bus. Server only
      renders the initial badge on first paint.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/mod.rs server/templates/partials/mention_badge.html server/templates/partials/sidebar.html`

### Task 9 - The notification bus: title flash, favicon, sound, browser API

- [ ] Add static assets:
  - `server/assets/favicon.svg` - keep a simple square chat-bubble SVG
    (one `<rect>` + one `<circle>` is fine).
  - `server/assets/favicon-dot.svg` - same plus a red `<circle r="3">` in
    the corner.
  - `server/assets/notify.ogg` - a 0.5s synthesised tone, ~2 KiB. If
    sourcing one is annoying, use the
    `https://github.com/cuckoolanding/freesound` sample at attribution-free
    license; otherwise omit and let Task 11's setting checkbox no-op when
    the file is missing.
- [ ] Edit `server/templates/base.html`:

```html
<link id="lc-favicon" rel="icon" href="/assets/favicon.svg?v={{ asset_version }}">
```

      (Replace any existing `<link rel="icon">` line.)
- [ ] Edit `server/templates/layout.html`. After the existing
      `#status-events-slot` div, add:

```html
<div id="lc-notify-bus" class="hidden"></div>
<div id="lc-mention-counts" class="hidden"
     data-base-title="lets-chat"
     data-browser-enabled="{% if user.notify_browser_enabled %}1{% else %}0{% endif %}"
     data-sound-enabled="{% if user.notify_sound_enabled %}1{% else %}0{% endif %}"></div>
```

  The `lc-mention-counts` element is used by JS as a per-page handle for
  user settings + base title. (`layout.html` already has a `user` context
  passed via `{% block %}` chain - confirm; if not, thread one in.)
- [ ] In the same template, add the bus controller. Hard cap: 60 lines.

```html
<script>
(function(){
  var bus = document.getElementById('lc-notify-bus');
  var cfg = document.getElementById('lc-mention-counts');
  if (!bus || !cfg) return;
  var baseTitle = cfg.getAttribute('data-base-title') || document.title;
  var browserEnabled = cfg.getAttribute('data-browser-enabled') === '1';
  var soundEnabled  = cfg.getAttribute('data-sound-enabled')   === '1';
  var counts = {};                  // room_id -> unread mention count
  var permPrompted = false;
  function totalCount(){ var n=0; for (var k in counts) n += counts[k]; return n; }
  function refreshTitle(){
    var n = totalCount();
    document.title = n > 0 ? '(' + n + ') ' + baseTitle : baseTitle;
  }
  function refreshFavicon(){
    var n = totalCount();
    var link = document.getElementById('lc-favicon');
    if (!link) return;
    link.href = n > 0 ? '/assets/favicon-dot.svg' : '/assets/favicon.svg';
  }
  function isOnRoom(target){
    return location.pathname === target;
  }
  function fireNotification(d){
    if (!browserEnabled) return;
    if (!('Notification' in window)) return;
    if (Notification.permission === 'denied') return;
    if (Notification.permission !== 'granted') {
      if (!permPrompted) {
        permPrompted = true;
        Notification.requestPermission();
      }
      return;
    }
    var title = d.kind === 'dm' ? 'Direct message from ' + d.author : d.author + ' mentioned you in ' + d.roomLabel;
    var n = new Notification(title, { body: d.snippet, tag: 'lc-' + d.target });
    n.onclick = function(){ window.focus(); location.assign(d.target); n.close(); };
  }
  function playSound(){
    if (!soundEnabled) return;
    try { new Audio('/assets/notify.ogg').play(); } catch (e) {}
  }
  function process(node){
    var ev = node.getAttribute('data-event');
    var roomId = node.getAttribute('data-room-id');
    if (ev === 'mentioned') {
      counts[roomId] = (counts[roomId] || 0) + 1;
      var d = {
        kind: node.getAttribute('data-kind'),
        roomLabel: node.getAttribute('data-room-label'),
        author: node.getAttribute('data-author'),
        snippet: node.getAttribute('data-snippet'),
        target: node.getAttribute('data-target'),
      };
      var bg = document.hidden || !isOnRoom(d.target);
      if (bg) { fireNotification(d); playSound(); }
      var b = document.getElementById('mention-room-' + roomId);
      if (b) {
        var n = counts[roomId];
        b.outerHTML = '<span id="mention-room-' + roomId + '" class="ml-1 text-[10px] font-bold uppercase bg-red-600 text-white rounded px-1.5">@' + n + '</span>';
      }
    } else if (ev === 'mention_cleared') {
      if (counts[roomId]) counts[roomId] = Math.max(0, counts[roomId] - 1);
    }
    refreshTitle();
    refreshFavicon();
  }
  new MutationObserver(function(muts){
    muts.forEach(function(m){
      Array.prototype.forEach.call(m.addedNodes, function(n){
        if (n.nodeType === 1) process(n);
      });
    });
    bus.replaceChildren();
  }).observe(bus, { childList: true });
  // Re-fetch initial counts via page render; counts{} starts empty and
  // hydrates as events arrive. The sidebar badge is server-rendered on
  // first paint, so no client-side hydration is required.
})();
</script>
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/templates/base.html server/templates/layout.html server/assets/favicon.svg server/assets/favicon-dot.svg server/assets/notify.ogg`

### Task 10 - Settings UI

- [ ] Edit `server/src/routes/settings.rs`. Extend `SettingsForm`:

```rust
#[serde(default)]
pub notify_browser_enabled: Option<String>,
#[serde(default)]
pub notify_sound_enabled: Option<String>,
```

  In `post_settings` after `set_read_receipts_enabled` and
  `set_profile_public`:

```rust
let browser = form.notify_browser_enabled.is_some();
let sound = form.notify_sound_enabled.is_some();
db::auth::set_notification_prefs(&state.auth, &user.id, browser, sound).await?;
```

- [ ] Edit `server/templates/settings/page.html`. Inside the
      `<form method="post" action="/settings">` block, after the
      `read_receipts_enabled` checkbox:

```html
<label class="flex items-center gap-2 cursor-pointer">
  <input type="checkbox" name="notify_browser_enabled" value="1" {% if user.notify_browser_enabled %}checked{% endif %}>
  <span>Show browser notifications when I am @mentioned or DM'd</span>
</label>
<label class="flex items-center gap-2 cursor-pointer">
  <input type="checkbox" name="notify_sound_enabled" value="1" {% if user.notify_sound_enabled %}checked{% endif %}>
  <span>Play a sound on new mentions and DMs</span>
</label>
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/settings.rs server/templates/settings/page.html`

### Task 11 - DB integration tests

- [ ] Create `server/tests/db_mentions.rs`:

```rust
use sqlx::SqlitePool;

async fn setup() -> (SqlitePool, SqlitePool) {
    // Use the same migration list as `db_read_receipts.rs`, plus 0012,
    // 0013, 0014. Auth side: 0001-0007.
    // (See db_read_receipts.rs for the exact pattern.)
    todo!("copy from db_read_receipts.rs and append 0012/0013/0014")
}

#[tokio::test]
async fn parse_extracts_distinct_tokens() {
    let tokens = lets_chat::db::mentions::parse_mention_tokens(
        "hi @alice, please cc @bob and @alice again");
    assert_eq!(tokens, vec!["alice".to_string(), "bob".to_string()]);
}

#[tokio::test]
async fn parse_ignores_email_addresses() {
    // `foo@bar.com` should NOT yield a `bar` token, because the `@` is not
    // preceded by start-of-string or whitespace. (Confirms TOKEN_PATTERN
    // anchoring once we add it.)
    let tokens = lets_chat::db::mentions::parse_mention_tokens("ping foo@bar.com");
    assert_eq!(tokens, Vec::<String>::new());
}

// reconcile_mentions: insert added, delete removed, idempotent on no-op
#[tokio::test]
async fn reconcile_inserts_and_removes() { /* ... */ }

// mark_mentions_read_for_room: only updates rows <= watermark
#[tokio::test]
async fn watermark_advances_read_state() { /* ... */ }

// count_unread_mentions_per_room: groups by room, ignores read rows
#[tokio::test]
async fn unread_counts_excludes_read() { /* ... */ }
```

  The plan author should keep `parse_ignores_email_addresses` honest: the
  current `TOKEN_PATTERN` does NOT anchor on `(^|\s)`. Update
  `parse_mention_tokens` to require the boundary OR remove this test. The
  composer-side regex already requires `(^|\s)` for the cursor token, so
  the parser should match.

  **Action**: change `TOKEN_PATTERN` in `db::mentions::parse_mention_tokens`
  to `(?:^|\s)@([A-Za-z0-9_-]{1,32})` and adjust the capture group index.
- [ ] `./dev/cargo test -p lets-chat-server --test db_mentions`
- [ ] `git add server/tests/db_mentions.rs server/src/db/mentions.rs`

### Task 12 - HTTP integration tests

- [ ] Create `server/tests/routes_mentions.rs`. Mirror the
      `routes_uploads.rs::app_with_user` helper for pool setup. Cases:
  - `GET /users/mentions?room_id=R&q=ali` returns a `<button data-username="alice">` when `alice` is a member of R; same query returns nothing when caller is not in R.
  - `POST /room/{R}/messages` body=`@alice hi` inserts a row in `mentions` and broadcasts to alice. (Verify via DB; broadcast verification is out of scope without a WS test harness.)
  - `PATCH /messages/{M}` body=`hi alice` (no `@`) removes the previous `mentions` row.
  - DM send: `POST /room/{DM}/messages` body=`yo` does NOT insert into `mentions`; broadcast goes via `ChatEvent::Mentioned { kind: "dm" }` (assert via a unit test on the helper that builds the event).
  - Cross-room mention is dropped: posting `@alice` in a private room alice is not a member of leaves `mentions` empty.
  - Self-mention is dropped: posting `@me` as me leaves `mentions` empty.
- [ ] `./dev/cargo test -p lets-chat-server --test routes_mentions`
- [ ] `git add server/tests/routes_mentions.rs`

### Task 13 - Final verification

- [ ] `just check-server`
- [ ] `just check-clippy` (apply `-D warnings` if any new warnings).
- [ ] `just test`
- [ ] `just check-fmt` (run `just fmt` if needed).
- [ ] `just verify`.
- [ ] Manual smoke (`just dev-web-local`):
  1. Open two browsers as users A and B.
  2. In #general, B sends `@A hello`. A's room view shows the message with
     a styled `@A` chip. A's title shows `(1) lets-chat`. A's favicon
     gains the red dot. A's sidebar `#general` row shows a red `@1` badge.
  3. While A's tab is hidden, B sends another `@A`. A's OS receives a
     browser notification (after granting permission once); clicking it
     focuses the tab and routes to `/room/{id}`.
  4. A opens `#general`. The badge clears, the title resets, the favicon
     swaps back. The DB row in `mentions` for the new message has
     `read_at IS NOT NULL`.
  5. B edits the message, removing the `@A`. A's mention count
     decrements; sidebar `@1` disappears.
  6. B sends `@A` again, then deletes the message. A's count decrements.
  7. B sends a DM to A. A sees a notification surface identical to a
     mention; sidebar shows the existing DM unread badge (no `@`
     mention badge - DMs don't write mention rows).
  8. A toggles "Show browser notifications" off in settings. Future
     mentions only flash the title, no OS notification.
  9. A toggles sound on. Future hidden-tab mentions play the tone.
  10. Email-shaped strings (`foo@bar.com`) do NOT render as a mention chip
      and do NOT fire a notification.
  11. Composer autocomplete: typing `@a` shows alice in the popover;
      ArrowDown / Enter inserts `@alice`; Escape cancels; clicking a
      suggestion inserts it; Enter without an open popover still sends
      the message (regression check on the existing keyboard handler).
- [ ] Verify diff is clean: `git status` shows only the expected files.
- [ ] **STOP. Do not commit. Do not push.** The user reviews the staged
      diff and creates the commit themselves.

## Out of scope (explicit)

- Web Push, service workers, VAPID, push subscriptions table.
- Email digest of missed mentions.
- Per-room mute / per-DM mute / per-user notification preferences.
- `@here`, `@channel`, `@everyone`.
- Mobile-native push (separate from Web Push - Tauri/Capacitor wrappers).
- Attempting to recall a browser notification once fired (the OS owns it
  after `new Notification(...)`).
- Mentioning users in *thread replies* triggers the same flow as a
  top-level message (reuse `post_message` path; threads don't need a
  separate mention extraction step). Not separately tested here.
- Smart mention semantics like "mark the original mention read when the
  user clicks the in-app message link" - the existing watermark-driven
  `mark_mentions_read_for_room` covers this when the user opens the
  room.

## Things to confirm / deviations

- **Migration numbers**: chat **`0014`**, auth **`0007`**.
- **`db::auth::list_user_ids`** and **`db::enclave::list_member_ids`** may
  not yet exist as 5-line wrappers. Add them as needed in Task 4 - they
  are mechanical.
- **Layout `user` context**: confirm that `layout.html` already has a
  `user` available (it does today via the per-page page structs that
  include `user: &User`). The new `data-browser-enabled` attribute
  depends on it.
- **Existing composer keyboard handler**: the new mention keydown
  listener and the existing Enter-to-send listener both attach to the
  textarea. Confirm during the manual smoke that Enter still sends when
  no popover is open. If it doesn't, gate the send handler with a
  `if (document.querySelector('#lc-mention-list')) return;` early-return.
- **Asset cache busting**: existing `?v={{ asset_version }}` query strings
  cover the new favicon and notify.ogg as long as they're served from
  `server/assets/`. Confirm.
- **`UserRecord` migration spread**: every test file that builds an auth
  pool registers migrations 0001-0006 today. Adding 0007 is required to
  keep the new `notify_*` columns selectable. Task 1 enumerates the
  files; the actual edit is one line per file.
- **DM `Mentioned` event uses `target_path = "/dm/{author_id}"`**: the
  recipient navigates to the *author's* DM (the route is symmetric;
  either user id resolves the same DM room).
- **No `MentionAdded` for already-existing edited rows**: if a user
  edits a message and re-adds the same `@alice`, `reconcile_mentions`
  no-ops and no event fires. Acceptable; the original notification
  already fired.
- **`render_body` ordering**: mention-chip substitution must run BEFORE
  linkify. Otherwise `@foo.com` becomes a URL anchor first and the
  mention pass never sees it. Task 11 covers this with a parser test;
  add a corresponding `body_html` rendering test too if time permits.
