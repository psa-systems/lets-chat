# Phase 15 - Per-Room Mute

## Goal

Let any signed-in user mute notifications for a specific room. The mute setting
is per `(user, room)` and has three modes:

- **`none`** - default. All notification surfaces fire normally (sidebar
  unread bump, sidebar `@N` chip, title flash, sound, browser Notification,
  and - in the next phase - Web Push).
- **`except_mentions`** - the room is silent for ordinary new messages: the
  sidebar unread bump is suppressed, the title flash and sound do not fire,
  no browser Notification is shown. Explicit `@username` mentions still
  fire every notification surface and still bump the sidebar `@N` chip.
- **`all`** - the room is fully silent. Even direct `@username` mentions
  produce no notification surface and no badge bump.

The toggle lives on a small disclosure popover anchored to the room header
title. Muted rooms render with a greyed name in the sidebar and a hidden
unread badge; the mention chip remains vivid red so `except_mentions` users
still see real pings at a glance. Mute is forward-looking: pre-existing
unread mentions in a room are not zeroed when a user mutes the room; opening
the room clears them through the existing read-watermark path.

Out of scope (deferred to later phases):

- **Per-DM mute.** A DM-peer mute lives on the peer profile and is a
  different surface; this phase's `Mentioned` filter explicitly bypasses
  the mute check for DM-kind events so DM unread state stays canonical.
- **Time-bound mute** (mute for 1 hour / until tomorrow). The schema
  reserves a nullable `muted_until TEXT` column for the future phase but
  this phase does not read or write it - no helper, no UI, no test.
- **Sound-only mute** as a fourth mode. The global `notify_sound_enabled`
  user preference (Phase 14) already serves the open-plan-office case.
- **Admin-set unmutable rooms** / "you cannot mute the announcements
  channel" overrides. Mute is purely per-user.
- **Sidebar reordering** (collapsing muted rooms into a separate group).
- **Push integration.** Web Push is the next phase; the helper signature
  (`db::notifications::room_mute_mode`) is the shared seam.

## Architecture

- **Stack** (current truth): Axum 0.8 + Askama + HTMX. WebSocket payloads
  are pre-rendered HTML fragments tagged with `hx-swap-oob`; never JSON.
- **Schema (`chat.db`).** New table
  `room_notification_settings(user_id, room_id, mute_mode, muted_until,
  updated_at)` with PK `(user_id, room_id)` and a CHECK constraint on the
  `mute_mode` enum. Absence of a row means `mute_mode = 'none'`. Setting
  the mode to `none` deletes the row, not stores `'none'`, so the
  default-valued (largest) state stays absent and the table only
  accumulates rows for users who actively muted something.
- **`muted_until` is forward-compat scaffolding only.** The column is
  added now to avoid a future migration but is not read or written by
  any code in this phase. No helper, no UI, no test. The future
  time-bound-mute phase will add the read path, the clock check, and
  the duration picker. (See the "Things to confirm" section if you
  disagree.)
- **`MuteMode` enum.** `enum MuteMode { None, ExceptMentions, All }`
  with `as_str()` / `parse_str()` boundary helpers. `None` is the
  in-memory default returned by the DB layer when no row exists.
- **`db::notifications` module (new).** Houses every persistence and
  predicate function for notification preferences. Today: just mute. The
  next phase adds Push subscription storage to the same module so the
  fan-out paths only ever import from `db::notifications`. Functions:
  - `room_mute_mode(user_id, room_id) -> MuteMode` (single-row lookup;
    used by the WS render path).
  - `room_mute_modes_for_user(user_id) -> HashMap<i64, MuteMode>` (bulk
    loader; used by `routes::load_sidebar` to populate
    `SidebarRoom::mute_mode` per page render).
  - `set_room_mute_mode(user_id, room_id, mode) -> Result<()>`. When
    `mode == None`, the function internally calls
    `delete_room_mute_setting`; otherwise it upserts a row and stamps
    `updated_at`.
  - `delete_room_mute_setting(user_id, room_id) -> Result<()>` (used
    internally by `set_room_mute_mode`; not part of the public route
    surface).
- **WS render-path filtering.** Two filters, both consulted via
  `db::notifications::room_mute_mode`:
  1. The `Mentioned` arm in `routes::ws::ws_handler`'s send loop
     (server/src/routes/ws.rs:131). Mute applies only to room-kind
     mentions; DM-kind mentions (`kind == "dm"`) bypass the check
     because DM mute is a separate phase. Effect: `mute_mode = all`
     suppresses room-kind `Mentioned`; `mute_mode = except_mentions`
     allows it through; `mute_mode = none` allows it through. DMs
     always pass.
  2. The badge-bump branch in `render_new_message_or_bump`
     (server/src/routes/ws.rs:355). Both `all` and `except_mentions`
     suppress the unread-badge bump for non-foreground recipients.
     The foreground render path (the viewer has the room open) is
     **not** filtered: when you're looking at a room, you see new
     messages regardless of your mute setting. The mark-as-read /
     `mark_mentions_read_for_room` calls also continue to fire so a
     muted-then-opened room properly clears its accumulated mentions.
- **`SidebarRoom::mute_mode`.** New `mute_mode: String` field
  (rendered as a string for template comparisons; Askama doesn't reach
  through Rust enums). Populated from
  `room_mute_modes_for_user` once per page render in
  `routes::load_sidebar`. The sidebar template applies a greyed
  `text-slate-400` class to the room link when `mute_mode != "none"`,
  and the included `partials/unread_badge.html` renders an empty
  placeholder when the room is muted regardless of unread count.
  The mention badge keeps its existing render logic - it still fires
  for `except_mentions` rooms.
- **Live sidebar refresh on mute change.** Setting a new mute mode is
  a per-user concern: it doesn't fan out to the whole room. The
  `POST /room/:id/notify-prefs` handler returns the swapped header
  partial inline (so the requesting tab updates immediately) and
  additionally broadcasts a per-user `RoomNotifyPrefsChanged` event to
  every WS connection of the same user (other tabs / desktop wrapper)
  via `Hub::broadcast_to_user`. The recipient's WS handler renders a
  full sidebar OOB swap (already implemented as `render_sidebar` for
  RoomMemberAdded/Removed) so muted-room styling and badge visibility
  flip live across tabs without a page reload.
- **Disclosure pattern for the room-header dropdown.** Use a button
  with proper ARIA (`aria-expanded`, `aria-controls`, `aria-haspopup`)
  and a sibling `<div role="menu">` containing three radio inputs.
  Inline JS (capped at 30 lines) wires:
  - Click on the button toggles `aria-expanded` and unhides the menu.
  - `Escape` while focus is inside the menu closes the menu and
    returns focus to the toggle button.
  - Click outside the menu closes it.
  - `Tab` cycles naturally through the radios (default browser
    behavior; no custom handler needed).
  Selecting a radio fires HTMX (`hx-post="/room/{id}/notify-prefs"`,
  `hx-trigger="change from:input"`, `hx-target="#lc-room-header"`,
  `hx-swap="outerHTML"`). The handler returns the freshly-rendered
  header so the radio reflects the persisted state.
- **DM is exempt.** The DM render path posts a `Mentioned` with
  `kind = "dm"`; the WS filter checks `kind` before consulting
  `room_mute_mode` and skips the lookup entirely for DM events. DM
  rooms therefore can't be muted via this UI: `routes/dm.rs::get_dm`
  does not render the notify dropdown (the dropdown lives in
  `room/page.html` only, not `dm/page.html`).

## Tech Stack

- New crates: none.
- New static assets: none.
- No new build steps; pure Rust + Askama + Tailwind classes already in the
  built stylesheet (`text-slate-400`, `bg-white`, `shadow-lg`, etc.).

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `server/migrations/chat/0015_room_notification_settings.sql` | Per `(user, room)` mute settings table. |
| Add | `server/src/db/notifications.rs` | `MuteMode` enum + four DB functions. New module. |
| Edit | `server/src/db/mod.rs` | `pub mod notifications;`. |
| Edit | `server/src/views/layout.rs` | Add `mute_mode: String` to `SidebarRoom`. |
| Edit | `server/src/routes/mod.rs` | Bulk-load mute modes in `load_sidebar`; populate `SidebarRoom::mute_mode`; register the `/room/:id/notify-prefs` route. |
| Add | `server/src/routes/notify_prefs.rs` | `POST /room/:id/notify-prefs` handler returning the swapped header. |
| Edit | `server/src/routes/room.rs` | Pass `mute_mode` into `RoomPage` so the header dropdown renders the right radio as checked. |
| Edit | `server/src/routes/ws.rs` | Filter `Mentioned` and `NewMessage`-bump by mute mode; render the new `RoomNotifyPrefsChanged` event by re-rendering the sidebar OOB. |
| Edit | `server/src/ws/events.rs` | Add `ChatEvent::RoomNotifyPrefsChanged { user_id, room_id, mute_mode }`. |
| Edit | `server/src/views/ws_fragments.rs` | Skip `RoomNotifyPrefsChanged` in `render_event` (handled inline). |
| Edit | `server/src/views/room.rs` | Add `mute_mode: String` to `RoomPage`. |
| Add | `server/src/views/notify_prefs.rs` | `RoomHeaderFragment` Askama struct - the swapped partial. |
| Add | `server/templates/room/notify_dropdown.html` | Disclosure button + radio menu + 25-line inline JS. |
| Add | `server/templates/partials/room_header.html` | Title + topic + dropdown wrapped in `id="lc-room-header"` so the POST swap-target is stable. |
| Edit | `server/templates/room/page.html` | Replace inline `<header>` with `{% include "partials/room_header.html" %}`. |
| Edit | `server/templates/partials/sidebar.html` | Apply `text-slate-400` when `room.mute_mode != "none"`; pass `mute_mode` to `unread_badge.html`. |
| Edit | `server/templates/partials/unread_badge.html` | Suppress badge render when `mute_mode != "none"`. |
| Add | `server/tests/db_notifications.rs` | DB tests for the four functions in `db::notifications`. |
| Add | `server/tests/routes_room_mute.rs` | Route + WS-filter integration tests. |
| Edit | every `tests/*.rs` that registers chat migrations | Append `0015_room_notification_settings.sql` to the migration list. |

## Tasks

### Task 1 - Schema, `MuteMode`, DB module

- [ ] Confirm next chat migration number: `ls server/migrations/chat/`
      currently ends at `0014_mentions.sql`, so the next is **`0015`**.
- [ ] Confirm next auth migration number is unchanged from phase 14
      (this phase touches no auth migrations).
- [ ] Create `server/migrations/chat/0015_room_notification_settings.sql`:

```sql
CREATE TABLE IF NOT EXISTS room_notification_settings (
    user_id      TEXT    NOT NULL,
    room_id      INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    mute_mode    TEXT    NOT NULL CHECK (mute_mode IN ('none', 'except_mentions', 'all')),
    muted_until  TEXT,
    updated_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, room_id)
);

CREATE INDEX IF NOT EXISTS idx_room_notify_settings_user
    ON room_notification_settings (user_id);
```

  Note: the table lives in `chat.db` (foreign key to `rooms`). The
  PK doubles as the `(user, room)` lookup index; the secondary index
  on `user_id` alone serves the bulk loader. `muted_until` is
  forward-compat scaffolding for the future time-bound-mute phase
  and is not read or written by any code in this phase.

- [ ] Create `server/src/db/notifications.rs`:

```rust
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// Per-room notification preference. Absence of a row in
/// `room_notification_settings` is treated as `MuteMode::None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteMode {
    None,
    ExceptMentions,
    All,
}

impl MuteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MuteMode::None => "none",
            MuteMode::ExceptMentions => "except_mentions",
            MuteMode::All => "all",
        }
    }

    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "none" => Some(MuteMode::None),
            "except_mentions" => Some(MuteMode::ExceptMentions),
            "all" => Some(MuteMode::All),
            _ => None,
        }
    }
}

/// Single-row lookup. Returns `MuteMode::None` if no row exists.
/// Called from the WS render path on every `Mentioned` / `NewMessage`
/// fan-out targeted at the recipient.
pub async fn room_mute_mode(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<MuteMode, sqlx::Error> {
    let row = sqlx::query(
        "SELECT mute_mode FROM room_notification_settings \
          WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => MuteMode::None,
        Some(r) => MuteMode::parse_str(r.get::<&str, _>("mute_mode"))
            .unwrap_or(MuteMode::None),
    })
}

/// Bulk loader for sidebar rendering. Returns rooms where the user has
/// a setting; rooms without rows are absent from the map and the caller
/// substitutes `MuteMode::None`.
pub async fn room_mute_modes_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<HashMap<i64, MuteMode>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT room_id, mute_mode FROM room_notification_settings \
          WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let id: i64 = r.get("room_id");
            MuteMode::parse_str(r.get::<&str, _>("mute_mode"))
                .map(|m| (id, m))
        })
        .collect())
}

/// Upsert the mute mode. `MuteMode::None` removes the row instead of
/// writing the literal `'none'` so absence-default stays the schema
/// invariant - empty table = nobody has muted anything.
pub async fn set_room_mute_mode(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    mode: MuteMode,
) -> Result<(), sqlx::Error> {
    if matches!(mode, MuteMode::None) {
        return delete_room_mute_setting(pool, user_id, room_id).await;
    }
    sqlx::query(
        "INSERT INTO room_notification_settings \
             (user_id, room_id, mute_mode, updated_at) \
         VALUES (?, ?, ?, datetime('now')) \
         ON CONFLICT(user_id, room_id) DO UPDATE SET \
             mute_mode = excluded.mute_mode, \
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(mode.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_room_mute_setting(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM room_notification_settings \
          WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] Add `pub mod notifications;` to `server/src/db/mod.rs` (alphabetical
      placement between `mentions` and `moderation`).
- [ ] Register `0015` in every test file that opens a chat pool. Affected
      files (confirmed via `grep -lE "include_str!\(\"\.\./migrations/chat"
      server/tests/*.rs`):

  - `server/tests/db_dm.rs`
  - `server/tests/db_enclave.rs`
  - `server/tests/db_mentions.rs`
  - `server/tests/db_moderation.rs`
  - `server/tests/db_private_rooms.rs`
  - `server/tests/db_reactions.rs`
  - `server/tests/db_read_receipts.rs`
  - `server/tests/db_search.rs`
  - `server/tests/db_uploads.rs`
  - `server/tests/last_visited.rs`
  - `server/tests/message_editing.rs`
  - `server/tests/message_grouping.rs`
  - `server/tests/migration_enclaves.rs`
  - `server/tests/routes_enclave.rs`
  - `server/tests/routes_mentions.rs`
  - `server/tests/routes_uploads.rs`

  In each, append to the chat-migrations list:

```rust
include_str!("../migrations/chat/0015_room_notification_settings.sql"),
```

  Place it after `0014_mentions.sql` (or at the end of the existing
  list - the order matches filename order on disk).

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git checkout -b feat/room-mute`
- [ ] `git add server/migrations/chat/0015_room_notification_settings.sql server/src/db/notifications.rs server/src/db/mod.rs server/tests/`

### Task 2 - DB tests

- [ ] Create `server/tests/db_notifications.rs`:

```rust
use lets_chat::db::notifications::{
    self, room_mute_mode, room_mute_modes_for_user, set_room_mute_mode, MuteMode,
};
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
        include_str!("../migrations/chat/0007_reactions.sql"),
        include_str!("../migrations/chat/0008_search.sql"),
        include_str!("../migrations/chat/0009_enclaves.sql"),
        include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
        include_str!("../migrations/chat/0011_threads.sql"),
        include_str!("../migrations/chat/0014_mentions.sql"),
        include_str!("../migrations/chat/0015_room_notification_settings.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn seed_room(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, 'public')")
        .bind(name)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn lookup_returns_none_when_no_row() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn set_and_lookup_round_trip() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
}

#[tokio::test]
async fn upsert_overwrites_existing_mode() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    set_room_mute_mode(&pool, "user-1", r, MuteMode::All)
        .await
        .unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
}

#[tokio::test]
async fn setting_to_none_deletes_row() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    set_room_mute_mode(&pool, "user-1", r, MuteMode::All)
        .await
        .unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::None)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind("user-1")
    .bind(r)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn delete_helper_is_idempotent() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    notifications::delete_room_mute_setting(&pool, "user-1", r)
        .await
        .unwrap();
    notifications::delete_room_mute_setting(&pool, "user-1", r)
        .await
        .unwrap();
}

#[tokio::test]
async fn bulk_loader_returns_only_set_rooms() {
    let pool = setup_chat_pool().await;
    let r1 = seed_room(&pool, "alpha").await;
    let r2 = seed_room(&pool, "beta").await;
    let _r3 = seed_room(&pool, "gamma").await;
    set_room_mute_mode(&pool, "user-1", r1, MuteMode::All)
        .await
        .unwrap();
    set_room_mute_mode(&pool, "user-1", r2, MuteMode::ExceptMentions)
        .await
        .unwrap();
    let map = room_mute_modes_for_user(&pool, "user-1").await.unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&r1), Some(&MuteMode::All));
    assert_eq!(map.get(&r2), Some(&MuteMode::ExceptMentions));
}

#[tokio::test]
async fn check_constraint_rejects_unknown_mode() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    let res = sqlx::query(
        "INSERT INTO room_notification_settings (user_id, room_id, mute_mode) \
         VALUES (?, ?, ?)",
    )
    .bind("user-1")
    .bind(r)
    .bind("bogus")
    .execute(&pool)
    .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn parse_str_known_values_round_trip() {
    for m in [MuteMode::None, MuteMode::ExceptMentions, MuteMode::All] {
        assert_eq!(MuteMode::parse_str(m.as_str()), Some(m));
    }
    assert!(MuteMode::parse_str("nope").is_none());
}
```

- [ ] `./dev/cargo test -p lets-chat-server --test db_notifications`
- [ ] `git add server/tests/db_notifications.rs`

### Task 3 - `SidebarRoom::mute_mode` plumbing

- [ ] Edit `server/src/views/layout.rs`. Add `pub mute_mode: String,` to
      `SidebarRoom`:

```rust
pub struct SidebarRoom {
    pub id: i64,
    pub name: String,
    pub unread: i64,
    pub mentions: i64,
    pub mute_mode: String,
    pub active: bool,
}
```

- [ ] Edit `server/src/routes/mod.rs::load_sidebar`. After the existing
      `mention_counts` block, bulk-load mute modes:

```rust
let mute_modes: HashMap<i64, lets_chat_server_internal_only_macro_alias_remove_me_from_finals>::new();
```

  In the actual edit, use the resolved type:

```rust
use crate::db::notifications::{room_mute_modes_for_user, MuteMode};

let mute_modes: HashMap<i64, MuteMode> =
    room_mute_modes_for_user(&state.chat, &user.id).await?;
```

  And in the `SidebarRoom { ... }` literal inside the `.map(|r| ...)`,
  set `mute_mode`:

```rust
mute_mode: mute_modes
    .get(&r.id)
    .copied()
    .unwrap_or(MuteMode::None)
    .as_str()
    .to_string(),
```

  Audit any other `SidebarRoom { ... }` construction sites with
  `grep -rn "SidebarRoom {" server/src/`. As of phase 14 there is only
  the one in `routes/mod.rs::load_sidebar`. If a new construction
  surfaces during this audit, set `mute_mode: "none".to_string()` for
  that path.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/views/layout.rs server/src/routes/mod.rs`

### Task 4 - `ChatEvent` + WS render-path filtering

- [ ] Edit `server/src/ws/events.rs`. Add to `ChatEvent`:

```rust
/// Per-user notification of a notify-prefs change. Recipients re-render
/// their sidebar OOB so the muted-room class flips and badges hide/show
/// across all of their open tabs. Routed via
/// `Hub::broadcast_to_user(user_id, ...)`.
RoomNotifyPrefsChanged {
    user_id: String,
    room_id: i64,
    mute_mode: String,
},
```

- [ ] Edit `server/src/views/ws_fragments.rs::render_event`. Add
      `RoomNotifyPrefsChanged` to the not-rendered match arms list (it
      is rendered per-recipient in `routes/ws.rs`).
- [ ] Edit `server/src/routes/ws.rs`:

  1. In the send loop's `match &e` block, add the
     `RoomNotifyPrefsChanged` arm just before the catch-all
     `_ => render_event(&e)` (server/src/routes/ws.rs:139). It
     re-renders the sidebar for the recipient when the event is
     addressed to them:

```rust
ChatEvent::RoomNotifyPrefsChanged { user_id, .. }
    if user_id == &send_user.id =>
{
    render_sidebar(&send_state, &send_user).await
}
```

  2. Tighten the existing `Mentioned` arm
     (server/src/routes/ws.rs:131) to consult the mute mode for
     non-DM kinds. Replace:

```rust
ChatEvent::Mentioned {
    mentioned_user_id, ..
} if mentioned_user_id == &send_user.id => render_mentioned(&e),
```

     with:

```rust
ChatEvent::Mentioned {
    mentioned_user_id,
    kind,
    room_id,
    ..
} if mentioned_user_id == &send_user.id => {
    use crate::db::notifications::{room_mute_mode, MuteMode};
    if kind == "dm" {
        render_mentioned(&e)
    } else {
        match room_mute_mode(&send_state.chat, &send_user.id, *room_id).await {
            Ok(MuteMode::All) => None,
            _ => render_mentioned(&e),
        }
    }
}
```

     `MuteMode::ExceptMentions` falls through to the default arm and
     `render_mentioned` fires - that's the whole point of the mode.

  3. In `render_new_message_or_bump` (server/src/routes/ws.rs:355),
     after the foreground `is_subscribed` branch but before the
     `render_unread_badge` call, suppress the bump when the room is
     muted. Add right above
     `render_unread_badge(state, viewer, &room).await`:

```rust
let mode = crate::db::notifications::room_mute_mode(
    &state.chat,
    &viewer.id,
    message.room_id,
)
.await
.unwrap_or(crate::db::notifications::MuteMode::None);
if matches!(
    mode,
    crate::db::notifications::MuteMode::All
        | crate::db::notifications::MuteMode::ExceptMentions
) {
    return None;
}
```

     The foreground branch (`is_subscribed`) is left untouched: viewers
     with the room open still see new messages and still advance their
     read watermark.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/ws/events.rs server/src/views/ws_fragments.rs server/src/routes/ws.rs`

### Task 5 - Sidebar template: greyed muted rooms + suppressed badge

- [ ] Edit `server/templates/partials/sidebar.html`. In the `Rooms`
      section, change the room `<a>` opening tag to apply a greyed
      class when `room.mute_mode != "none"`. Replace:

```html
<a href="/room/{{ room.id }}"{% if room.active %} aria-current="page"{% endif %} class="flex items-center px-2 py-1 rounded hover:bg-slate-200 focus:bg-slate-200 focus:outline-none focus:ring-2 focus:ring-blue-500{% if room.active %} bg-blue-100 font-semibold text-blue-900{% endif %}">
```

  with:

```html
<a href="/room/{{ room.id }}"{% if room.active %} aria-current="page"{% endif %} class="flex items-center px-2 py-1 rounded hover:bg-slate-200 focus:bg-slate-200 focus:outline-none focus:ring-2 focus:ring-blue-500{% if room.active %} bg-blue-100 font-semibold text-blue-900{% else if room.mute_mode != "none" %} text-slate-400{% endif %}">
```

      The `else if` ordering keeps the active-room highlight wins over
      the muted-grey style.

- [ ] In the same loop, pass `mute_mode` to `unread_badge.html`. Just
      above the existing `{% include "partials/unread_badge.html" %}`,
      add:

```html
{% let mute_mode = room.mute_mode.as_str() %}
```

      The mention badge include below is unchanged (mention chip still
      fires for `except_mentions` rooms).

- [ ] Edit `server/templates/partials/unread_badge.html`. Suppress the
      number chip when the surrounding context defines a `mute_mode`
      that is not `"none"`. The existing template already references
      `unread`, `kind`, `id` from `let` bindings; adding an optional
      `mute_mode` binding works the same way. New body:

```html
{% if unread > 0 && mute_mode == "none" %}
<span id="unread-{{ kind }}-{{ id }}" class="ml-auto text-xs bg-blue-600 text-white rounded px-2">{{ unread }}</span>
{% else %}
<span id="unread-{{ kind }}-{{ id }}"></span>
{% endif %}
```

      Every existing call site that includes `partials/unread_badge.html`
      must define `mute_mode`. Audit:

```bash
grep -rn 'unread_badge.html' server/templates/
```

      For DM unread badges (where mute does not apply), inject
      `{% let mute_mode = "none" %}` just above the include. Concretely:

  - In `server/templates/partials/sidebar.html`, the DM section already
    gets a fresh `let kind = "dm"` block; add `{% let mute_mode = "none" %}`
    next to it.
  - In `server/templates/ws/unread_badge.html` (the WS OOB swap), the
    template already takes `kind`/`id`/`unread` via the
    `UnreadBadgeFragment` Askama struct; add a `mute_mode: &'a str`
    field to the struct (default `"none"`) and template variable. Hard
    set `mute_mode: "none"` at every call site that constructs
    `UnreadBadgeFragment` (chat unread bumps already filter via the
    mute check in Task 4, so by the time the OOB swap is rendered the
    room is known to be unmuted - passing `"none"` is safe and keeps
    the template logic shared).

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/templates/partials/sidebar.html server/templates/partials/unread_badge.html server/templates/ws/unread_badge.html server/src/views/ws_fragments.rs`

### Task 6 - Room header partial + notify dropdown

- [ ] Create `server/templates/partials/room_header.html`. The wrapper
      id `lc-room-header` is the swap target the POST handler returns.

```html
<header id="lc-room-header" class="border-b border-slate-200 px-4 py-2 flex items-start justify-between gap-2">
  <div class="min-w-0">
    <h1 class="font-semibold truncate">#{{ room.name }}</h1>
    {% if let Some(topic) = room.topic.as_ref() %}
    <p class="text-sm text-slate-500 truncate">{{ topic }}</p>
    {% endif %}
  </div>
  {% include "room/notify_dropdown.html" %}
</header>
```

- [ ] Create `server/templates/room/notify_dropdown.html`. Hard cap: 30
      lines of inline JS.

```html
<div class="relative shrink-0">
  <button type="button"
    id="lc-notify-toggle-{{ room.id }}"
    class="inline-flex items-center gap-1 rounded border border-slate-200 px-2 py-1 text-xs hover:bg-slate-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
    aria-haspopup="menu"
    aria-expanded="false"
    aria-controls="lc-notify-menu-{{ room.id }}">
    {% if mute_mode == "none" %}Unmuted
    {% else if mute_mode == "except_mentions" %}Muted (mentions on)
    {% else %}Muted{% endif %}
  </button>
  <div id="lc-notify-menu-{{ room.id }}"
    role="menu"
    aria-labelledby="lc-notify-toggle-{{ room.id }}"
    hidden
    class="absolute right-0 z-30 mt-1 w-64 rounded border border-slate-200 bg-white p-2 shadow-lg text-sm">
    <form hx-post="/room/{{ room.id }}/notify-prefs"
          hx-trigger="change from:input"
          hx-target="#lc-room-header"
          hx-swap="outerHTML"
          class="space-y-1">
      <label class="flex items-start gap-2 cursor-pointer p-1 hover:bg-slate-50 rounded">
        <input type="radio" name="mute_mode" value="none" {% if mute_mode == "none" %}checked{% endif %} class="mt-1">
        <span><span class="font-medium">Unmuted</span><br><span class="text-xs text-slate-500">All notifications.</span></span>
      </label>
      <label class="flex items-start gap-2 cursor-pointer p-1 hover:bg-slate-50 rounded">
        <input type="radio" name="mute_mode" value="except_mentions" {% if mute_mode == "except_mentions" %}checked{% endif %} class="mt-1">
        <span><span class="font-medium">Muted (mentions on)</span><br><span class="text-xs text-slate-500">Only @-mentions notify.</span></span>
      </label>
      <label class="flex items-start gap-2 cursor-pointer p-1 hover:bg-slate-50 rounded">
        <input type="radio" name="mute_mode" value="all" {% if mute_mode == "all" %}checked{% endif %} class="mt-1">
        <span><span class="font-medium">Muted</span><br><span class="text-xs text-slate-500">No notifications, even mentions.</span></span>
      </label>
    </form>
  </div>
</div>
<script>
(function(){
  // Disclosure pattern: toggle button + menu div with proper ARIA.
  // Tab cycles through radios via default browser behavior. Escape closes.
  // Click outside closes. After change, HTMX swaps #lc-room-header so the
  // entire dropdown is replaced; this script no-ops on its second run.
  var btn = document.getElementById('lc-notify-toggle-{{ room.id }}');
  var menu = document.getElementById('lc-notify-menu-{{ room.id }}');
  if (!btn || !menu || btn.dataset.lcWired === '1') return;
  btn.dataset.lcWired = '1';
  function close(){ btn.setAttribute('aria-expanded', 'false'); menu.hidden = true; }
  function open(){ btn.setAttribute('aria-expanded', 'true'); menu.hidden = false;
    var first = menu.querySelector('input[type=radio]:checked, input[type=radio]'); if (first) first.focus(); }
  btn.addEventListener('click', function(){ menu.hidden ? open() : close(); });
  menu.addEventListener('keydown', function(e){
    if (e.key === 'Escape') { e.preventDefault(); close(); btn.focus(); }
  });
  document.addEventListener('click', function(e){
    if (menu.hidden) return;
    if (!menu.contains(e.target) && e.target !== btn) close();
  });
})();
</script>
```

- [ ] Edit `server/templates/room/page.html`. Replace the inline
      `<header>` block with the partial include. The new `main` block
      reads:

```html
{% block main %}
<div class="flex flex-1 overflow-hidden">
  <div class="flex flex-col flex-1 overflow-hidden">
    {% include "partials/room_header.html" %}
    {% include "room/messages.html" %}
    <div id="typing" class="px-4 text-xs text-slate-500"></div>
    {% include "room/composer.html" %}
  </div>
  <aside id="thread-panel" class="hidden"></aside>
</div>
<script>
document.body.addEventListener('htmx:wsOpen', function(evt) {
  window.__lcWS = evt.detail.socketWrapper;
  evt.detail.socketWrapper.send(JSON.stringify({
    type: 'subscribe',
    room_id: {{ room.id }}
  }));
});
</script>
{% include "partials/auto_scroll.html" %}
{% endblock %}
```

      The DM page (`templates/dm/page.html`) is intentionally not
      changed: DM mute is a separate phase, so DMs continue to render
      a header without a notify dropdown.

  Accessibility note for reviewers (also relevant for any future
  rework): the dropdown is implemented as a real disclosure pattern.
  The button has `aria-haspopup="menu"`, `aria-expanded` toggling
  between `"true"` and `"false"`, and `aria-controls` pointing to the
  menu id. The menu is keyboard-navigable: focus moves to the first
  (or checked) radio when opened, Tab cycles through radios via
  default browser behavior, Space/Enter selects (also default), and
  Escape returns focus to the toggle button. Click-outside closes
  with focus left at the click target.

- [ ] Add `mute_mode: String` to `RoomPage` in
      `server/src/views/room.rs`. Construct it from the new DB lookup
      below; no other field on `RoomPage` is changed.
- [ ] Edit `server/src/routes/room.rs::get_room`. Just before the
      `RoomPage` literal, look up the viewer's mute mode for this
      room:

```rust
let mute_mode = db::notifications::room_mute_mode(&state.chat, &user.id, room_id)
    .await
    .unwrap_or(db::notifications::MuteMode::None)
    .as_str()
    .to_string();
```

      And pass it to `RoomPage`:

```rust
let page = RoomPage {
    user: &user,
    room: &room,
    sidebar_rooms: &sidebar_rooms,
    sidebar_peers: &sidebar_peers,
    switcher: &switcher,
    messages: &messages,
    asset_version: &state.asset_version,
    mute_mode: &mute_mode,
};
```

      Make `mute_mode` a `&str` field on `RoomPage` if that matches the
      existing borrowing pattern there - or `String` if other fields
      are owned. Match the existing convention of the surrounding
      struct.

- [ ] Add a `RoomHeaderFragment` Askama struct in
      `server/src/views/notify_prefs.rs` (new file):

```rust
use askama::Template;

use crate::models::Room;

#[derive(Template)]
#[template(path = "partials/room_header.html")]
pub struct RoomHeaderFragment<'a> {
    pub room: &'a Room,
    pub mute_mode: &'a str,
}
```

- [ ] Add `pub mod notify_prefs;` to `server/src/views/mod.rs`.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/templates/partials/room_header.html server/templates/room/notify_dropdown.html server/templates/room/page.html server/src/views/room.rs server/src/views/notify_prefs.rs server/src/views/mod.rs server/src/routes/room.rs`

### Task 7 - `POST /room/:id/notify-prefs` route

- [ ] Create `server/src/routes/notify_prefs.rs`:

```rust
use axum::extract::{Path, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::notify_prefs::RoomHeaderFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Deserialize)]
pub struct NotifyPrefsForm {
    pub mute_mode: String,
}

/// POST /room/:id/notify-prefs
///
/// Persist the viewer's mute mode for `room_id` and return the swapped
/// `#lc-room-header` fragment so the caller's tab updates inline. Other
/// open tabs of the same user receive a `RoomNotifyPrefsChanged` event
/// over WS and re-render their sidebar.
pub async fn post_notify_prefs(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<NotifyPrefsForm>,
) -> Result<Html, AppError> {
    // Resolve the room and gate on read access. Mute is a personal
    // setting but exposing it for rooms the caller can't see leaks
    // existence.
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // DM rooms can't have notification preferences set via this surface
    // (per-DM mute is its own future phase). The room/page.html template
    // is the only place that renders the dropdown, but harden the
    // handler anyway.
    if room.room_type == "dm" {
        return Err(AppError::BadRequest("DM mute is not supported".into()));
    }

    let mode = db::notifications::MuteMode::parse_str(&form.mute_mode)
        .ok_or_else(|| AppError::BadRequest(format!("invalid mute_mode: {}", form.mute_mode)))?;
    db::notifications::set_room_mute_mode(&state.chat, &user.id, room_id, mode).await?;

    // Fan out a per-user event so other tabs of the same user re-render
    // their sidebar (greyed-name + badge visibility flips). The
    // requesting tab's sidebar is updated indirectly via the same WS
    // path; the inline response below additionally swaps the header.
    let event = ChatEvent::RoomNotifyPrefsChanged {
        user_id: user.id.clone(),
        room_id,
        mute_mode: mode.as_str().to_string(),
    };
    state.hub.broadcast_to_user(&user.id, &event);

    let fragment = RoomHeaderFragment {
        room: &room,
        mute_mode: mode.as_str(),
    };
    html(&fragment)
}
```

- [ ] Edit `server/src/routes/mod.rs`. Add `mod notify_prefs;` near
      the other route module declarations and register the route in
      `build_router`:

```rust
.route("/room/{room_id}/notify-prefs", post(notify_prefs::post_notify_prefs))
```

      Place it directly under the existing
      `.route("/room/{room_id}/messages", post(room::post_message))`
      so the room-scoped routes are grouped.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/routes/notify_prefs.rs server/src/routes/mod.rs`

### Task 8 - Route + WS-filter integration tests

- [ ] Create `server/tests/routes_room_mute.rs`. The setup follows the
      pattern in `server/tests/routes_mentions.rs`: build a router with
      two seeded users (viewer + peer), open chat/auth/settings pools
      with the full migration list (including `0015`), and exercise
      the `POST /room/:id/notify-prefs` handler plus a Hub-level
      assertion that confirms muted rooms suppress events.

  Tests to include:

  1. `post_persists_mute_mode_and_returns_swapped_header`:
     - POST `mute_mode=all` to `/room/{id}/notify-prefs`.
     - Assert response body contains `id="lc-room-header"`, the
       button label is `"Muted"`, the `<input value="all">` is
       `checked`, and a row exists in `room_notification_settings`
       with `mute_mode='all'`.
  2. `post_with_none_deletes_row`:
     - Set to `all`, then POST `mute_mode=none`.
     - Assert no row in `room_notification_settings` for this
       (user, room) pair.
  3. `post_invalid_mode_returns_400`:
     - POST `mute_mode=bogus`. Assert 400.
  4. `post_unauthenticated_returns_redirect_or_401`:
     - POST without a session cookie. Assert the existing
       `AuthUser` rejection (mirrors other route tests).
  5. `post_to_dm_room_returns_400`:
     - Build a DM room between viewer and peer. POST
       `mute_mode=all`. Assert 400 with the "DM mute is not
       supported" body.
  6. `post_to_inaccessible_private_room_returns_403`:
     - Create a private room with viewer not in `room_members`.
       POST. Assert 403.
  7. `mute_all_suppresses_mention_render`:
     - Set `mute_mode=all` for viewer in room R. Spawn a
       `Hub`-bound test consumer for viewer's user-id channel.
       Have peer post `@viewer hi` in R. Assert no `Mentioned`
       fragment is delivered to viewer (or that `render_mentioned`
       returns `None` when the WS arm is exercised directly).
  8. `mute_except_mentions_passes_mention_through`:
     - Same setup, mode `except_mentions`. Peer posts `@viewer
       hello`. Assert a `Mentioned` fragment IS delivered.
  9. `mute_all_suppresses_unread_badge_bump`:
     - Set `mute_mode=all`. Peer posts a non-mention message in R
       while viewer's WS connection is **not** subscribed to R.
       Assert `render_new_message_or_bump` returns `None` (no
       badge bump).
  10. `mute_except_mentions_suppresses_unread_badge_bump`:
      - Same as above with `except_mentions`. Assert badge bump
        is suppressed (regular new-message badge does not fire;
        only the mention chip does).
  11. `foreground_render_is_not_filtered`:
      - Set `mute_mode=all`. Subscribe viewer's WS to R. Peer
        posts a message. Assert `render_new_message_or_bump`
        returns `Some(html)` containing the message body (the
        viewer is actively reading the room).
  12. `mute_does_not_block_dm_kind_mention`:
      - Set `mute_mode=all` on the DM-room id (via direct DB
        write, since the route refuses DM rooms). Send a
        `Mentioned { kind: "dm", ... }` through the WS arm.
        Assert `render_mentioned` is invoked - the DM bypass.
        This guards against future regressions where someone
        adds a DM-mute UI without revisiting the WS filter.
  13. `mark_mentions_read_still_runs_for_muted_room_open`:
      - Set `mute_mode=all`. Pre-seed a mention row in R for
        viewer. GET `/room/{id}` as viewer. Assert the mention
        row's `read_at` is no longer NULL. (Mute is forward-
        looking; opening a muted room still clears its
        accumulated mentions.)
  14. `sidebar_renders_muted_room_with_greyed_class`:
      - Set `mute_mode=all` on R. GET the home page (or any
        page that renders the sidebar with R visible). Assert
        the response HTML contains `text-slate-400` adjacent
        to the `href="/room/{id}"` link.
  15. `sidebar_unread_badge_hidden_for_muted_room`:
      - Have peer post 3 messages in R (so viewer accumulates
        unread). Set `mute_mode=all` for viewer. GET the home/
        sidebar page. Assert the response contains
        `<span id="unread-room-{id}"></span>` (empty span)
        rather than the badge with the count.
  16. `mention_badge_still_renders_in_except_mentions_mode`:
      - Set `mute_mode=except_mentions`. Pre-seed a mention row
        in R. GET the sidebar page. Assert the response
        contains `<span id="mention-room-{id}" ...>@1</span>`.

- [ ] `./dev/cargo test -p lets-chat-server --test routes_room_mute`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/tests/routes_room_mute.rs`

### Task 9 - Final verification

- [ ] `just check-server`
- [ ] `just check-server-saas` (mute is mode-agnostic; should compile in
      both modes since none of the touched files are gated by a feature
      flag).
- [ ] `just check-clippy`
- [ ] `just check-clippy-saas`
- [ ] `just check-fmt` (run `./dev/cargo fmt --all` if it complains).
- [ ] `just test`
- [ ] `just test-saas`
- [ ] `just verify`

- [ ] Manual smoke-test list (`just dev-web-local`, log in as two users
      in two browsers / windows):

  1. As user A in room R, click the notification button next to the
     room title. Confirm the disclosure opens, focus moves into the
     menu, three radios are present, and the current mode is
     selected. Tab through the radios with the keyboard. Press
     Escape - menu closes, focus returns to the button.
  2. Select "Muted (mentions on)". Confirm the button label changes,
     the menu closes, and (in another tab also logged in as A) the
     sidebar entry for R turns grey.
  3. As B, post a regular message in R. Confirm A sees no
     title-bar count change, no unread badge increment, and no
     sound/notification.
  4. As B, post `@A hi`. Confirm A's title flashes, A's favicon
     dot appears, A's sidebar mention chip increments, and (if
     hidden) A receives a browser notification.
  5. Set R to "Muted" (all). As B, post `@A try again`. Confirm A
     sees no notification surface and no badge change of any kind.
  6. Open R as A. Confirm the accumulated unread mention from step
     4 is cleared (the mention chip and favicon dot drop to zero).
  7. Set R back to "Unmuted". Confirm sidebar styling reverts and
     normal notifications resume.
  8. (DM regression check.) Open a DM with B. Confirm there is no
     notify dropdown in the DM header. Have B send a DM. Confirm A
     receives the notification surface as before. (Asserts DM mute
     is not silently broken.)

## Things to confirm

- **`muted_until` column up-front:** the plan adds the nullable column
  now per the request. The trade-off is one row of migration text and
  a slightly wider table that the schema-validation tests will see;
  zero runtime cost. If you'd rather keep migrations strictly
  proportional to the code that reads them and add the column in the
  time-bound-mute phase, drop the column from
  `0015_room_notification_settings.sql`. I lean with the user's call
  here - schema migrations on SQLite are cheap-but-not-free, and
  combining them is genuinely nice in review.

- **Whether the WS arm should `.await` an extra DB query per
  `Mentioned` / `NewMessage` event.** Each fan-out event already costs
  one or more DB queries to render (author lookup, attachments,
  mentions). Adding a `room_mute_mode` lookup is a single indexed
  primary-key hit on a tiny table, so the hot-path cost is negligible.
  No caching layer in this phase. If the future Push fan-out finds
  this hot, we cache mute-modes per WS connection at subscribe time -
  but that's a future concern.

- **Sidebar render on mute change uses the existing `render_sidebar`
  helper** (server/src/routes/ws.rs:696), which today swaps the entire
  `#sidebar` aside via `outerHTML`. That re-runs every per-room render
  in the sidebar - cheap for normal rooms-per-user counts. Confirm
  this is still acceptable; if not, we can swap a single
  `#room-link-{id}` partial later. Phase 14 already accepts the
  full-sidebar swap pattern, so this is consistent.
