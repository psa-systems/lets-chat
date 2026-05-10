# Phase 17 - Per-DM Mute

## Goal

Let any signed-in user mute notifications for a specific DM peer. The mute
setting is binary: muted or unmuted, per `(user, dm_room_id)`. Muted DMs
suppress every notification surface for the muter:

- The WS `Mentioned { kind: "dm", ... }` event is dropped before render, so
  the title flash, favicon dot, sound, and browser Notification do not fire.
- `push::dispatch` returns early before any Push send.
- The sidebar DM row renders with a greyed peer name and a hidden unread
  badge, mirroring phase 15's room-mute treatment.
- The DM messages keep arriving and the `last_read_message_id` watermark
  keeps advancing - mute is "silent", not "ignored".

The toggle lives in the DM-page header as a single `<label><input
type="checkbox" hx-post>`. No dropdown chrome, no JS controller, no new
keyboard handler. HTMX swaps the header fragment on toggle.

The phase reuses the existing `room_notification_settings` table from
phase 15 (DM rooms are rooms; the `(user_id, room_id)` key already covers
them). No migration in this phase.

Out of scope (deferred):

- **Per-peer block.** Already shipped; `db::auth::user_blocks` and the
  blocked-DM short-circuit in `routes/dm.rs` are independent of mute.
- **Time-bound mute** (`muted_until` column reserved, not used). Future
  phase will cover rooms and DMs together.
- **Do-not-disturb / quiet hours.** Own future phase.
- **Profile-page mute toggle.** Deferred secondary surface for a future
  block+mute UI cluster.
- **Notifying the peer that they have been muted.** Mute is a private
  per-user setting; no fan-out beyond the muter's own tabs.

## Architecture

- **Stack** (current truth): Axum 0.8 + Askama + HTMX. WebSocket payloads
  are pre-rendered HTML fragments tagged with `hx-swap-oob`; never JSON.

- **Schema reuse, no migration.** The phase 15 table
  `room_notification_settings(user_id, room_id, mute_mode, muted_until,
  updated_at)` already keys on `(user_id, room_id)` with a FK + cascade
  to `rooms`. A DM is a row in `rooms` with `room_type = 'dm'`, so a DM
  mute is just a row whose `room_id` happens to point at a DM room.
  Phase 17 adds zero columns, zero indexes, zero migrations.

- **Two states only.** DMs use `MuteMode::All` for muted and
  `MuteMode::None` for unmuted. `MuteMode::ExceptMentions` is rejected
  by both the DM setter and the DM toggle endpoint - every DM is
  implicitly directed at the recipient, so "mute except mentions" has
  no meaning. We do **not** add a fourth enum variant.

- **Symmetric strict guards on the setters.** Both DB setters validate
  the room kind against the operation:
  - `set_dm_mute(pool, user_id, dm_room_id, muted: bool)` rejects when
    the room is not a DM.
  - `set_room_mute_mode(pool, user_id, room_id, mode)` (existing) is
    tightened to reject when the room IS a DM.
  Each setter does a single indexed `SELECT room_type FROM rooms WHERE
  id = ?` once per write, returns `sqlx::Error::Protocol` on mismatch.
  This catches `set_room_mute_mode(user, dm_room_id, ExceptMentions)`
  at the helper boundary in addition to the route-layer check.
  Self-validating helpers, defense in depth.

- **Three DM-bypass guards removed.** Phase 16 left three sites that
  short-circuit the mute lookup for DM-kind events:
  1. `server/src/push/mod.rs:166-175` - the `kind != "dm"` guard around
     `room_mute_mode`.
  2. `server/src/routes/ws.rs:141-152` - the `if kind == "dm" { true }`
     short-circuit in the WS `Mentioned` arm.
  3. `server/src/routes/room.rs:362-367` - the DM branch of
     `post_message`. The call site needs no change: the `Mentioned` is
     fanned out unconditionally and filtered downstream by the WS arm
     (#2) and `push::dispatch` (#1). The `// FUTURE` comment is
     replaced with a one-line note that mute is honored downstream.

- **Suppression data flow.** When peer B sends a DM to muter A in DM
  room R:
  1. B's `post_message` inserts the message (chat.messages).
  2. `broadcast_room_message` fans out `NewMessage { is_dm: true }` to
     room subscribers. Foreground (A has the DM open) renders inline.
     Non-foreground recipients hit `render_new_message_or_bump`, which
     already calls `room_mute_mode(A, R).allows_unread_bump()` and
     returns `None` for `MuteMode::All` (server/src/routes/ws.rs:440).
     No code change needed in this arm.
  3. B's DM branch broadcasts `Mentioned { kind: "dm", ... }` to A.
     A's WS `Mentioned` arm now consults `room_mute_mode(A, R)` (the
     `kind == "dm"` short-circuit is removed) and returns `None` for
     `MuteMode::All`.
  4. B's DM branch calls `push::dispatch(A, &event)`. The `kind != "dm"`
     guard inside dispatch is removed; the same `room_mute_mode(A, R)`
     lookup runs and the function returns early on `MuteMode::All`.

  **State mutation audit (verified, not just hoped):** the DM receive
  path mutates **nothing** in A's state during fan-out.
  - No mention row is written for DM-kind events
    (server/src/routes/room.rs:302-303 `if room.room_type != "dm"`).
  - DM unread state is computed from `last_read_message_id <
    last_message_id`; A's watermark is updated only when A reads
    (server/src/routes/dm.rs:223), not when A receives.
  - Read receipts and last-seen timestamps update on read, not on
    receive.
  - Sidebar unread bumps are render concerns, not state writes.

  Suppression therefore leaves nothing dangling. A muted DM continues
  to accumulate as "unread" by virtue of the watermark; opening the DM
  later clears it through the existing read path. Mute = silent, not
  lost.

- **`POST /dm/:peer_id/mute` (new).** Matches the existing DM URL
  convention (`GET /dm/:peer_id` in routes/dm.rs:29). Form payload is a
  single boolean (`muted=on` from the checkbox; absence = off). The
  handler:
  1. Resolves the peer (404 if missing).
  2. Resolves the DM room via `find_dm_room` (404 if no DM exists yet).
  3. Verifies the caller is a member of the DM room (403 otherwise -
     defensive; only the two members can ever resolve to this room).
  4. Calls `set_dm_mute`.
  5. Broadcasts `ChatEvent::DmMuteChanged { dm_room_id, peer_user_id,
     muted }` to the caller's other WS connections.
  6. Returns the swapped DM-header fragment so the requesting tab
     updates inline.

  Block interaction: blocked-DM pages render `WelcomePage` with a
  message instead of the DM page (routes/dm.rs:54-67), so the toggle
  is unreachable from the UI. The handler does **not** add an extra
  block check: muting your own private setting for an already-existing
  DM is harmless even when blocked, and adding a block check here
  would couple the mute endpoint to a feature that already gates
  surface-side. (One-line note in the handler explains this.)

- **`ChatEvent::DmMuteChanged` (new variant).** Carries
  `{ dm_room_id: i64, peer_user_id: String, muted: bool }`. Routed via
  `Hub::broadcast_to_user(user_id, ...)` so only the muter's own tabs
  receive it (cross-tab consistency). The WS render arm re-renders the
  full sidebar OOB via `render_sidebar`, which picks up the new
  `mute_mode` value for the affected `SidebarPeer`.

  See the "Things to confirm" section: this is the explicit user
  decision over reuse of `RoomNotifyPrefsChanged`. The WS render arms
  for both events end up calling the same `render_sidebar` helper, so
  the case for reuse is real - flagged for the user's final review.

- **`SidebarPeer::mute_mode` plumbing.** New `mute_mode: String` field
  on `SidebarPeer`, populated from the same
  `room_mute_modes_for_user(user_id)` lookup that already feeds
  `SidebarRoom`. The map is keyed by `room_id`; for the DM case we look
  up by `room.id` (the DM room id). The sidebar template's peers
  section gains the same `else if peer.mute_mode != "none"` greyed-link
  treatment and `{% let mute_mode = peer.mute_mode.as_str() %}` to feed
  `unread_badge.html` (which already suppresses the badge when
  `mute_mode != "none"` - phase 15 wiring).

- **DM header partial.** Today `templates/dm/page.html` inlines the
  `<header>`. Phase 17 extracts it into
  `templates/partials/dm_header.html` so the POST handler can return a
  swapped fragment. The wrapper id `lc-dm-header` is the swap target.
  The new mute toggle is a single `<label>` containing an
  `<input type="checkbox" hx-post="/dm/{peer_id}/mute"
  hx-trigger="change" hx-target="#lc-dm-header" hx-swap="outerHTML"
  name="muted" {% if mute_mode == "all" %}checked{% endif %}>`.

## Tech Stack

- New crates: none.
- New static assets: none.
- New migrations: none.
- No new build steps; pure Rust + Askama + Tailwind classes already in
  the built stylesheet.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Edit | `server/src/db/notifications.rs` | Add `set_dm_mute`; tighten `set_room_mute_mode` with strict room-kind guard. |
| Edit | `server/src/views/layout.rs` | Add `mute_mode: String` to `SidebarPeer`. |
| Edit | `server/src/routes/mod.rs` | In `load_sidebar`'s DM branch, populate `SidebarPeer::mute_mode` from `room_mute_modes_for_user`. |
| Edit | `server/src/push/mod.rs` | Remove the `kind != "dm"` guard in `dispatch`. |
| Edit | `server/src/routes/ws.rs` | Remove the `kind == "dm"` short-circuit in the `Mentioned` arm; add `DmMuteChanged` render arm. |
| Edit | `server/src/routes/room.rs` | Replace the `// FUTURE` comment in the DM branch of `post_message` with a one-line note that mute is honored downstream. |
| Edit | `server/src/ws/events.rs` | Add `ChatEvent::DmMuteChanged { dm_room_id, peer_user_id, muted }`. |
| Edit | `server/src/views/ws_fragments.rs` | Skip `DmMuteChanged` in `render_event` (handled inline like `RoomNotifyPrefsChanged`). |
| Edit | `server/src/views/dm.rs` | Add `mute_mode: String` to `DmPage`. |
| Add | `server/src/views/dm_header.rs` | `DmHeaderFragment` Askama struct - the swapped partial. |
| Edit | `server/src/views/mod.rs` | `pub mod dm_header;`. |
| Add | `server/templates/partials/dm_header.html` | Avatar + name + block + mute toggle wrapped in `id="lc-dm-header"`. |
| Edit | `server/templates/dm/page.html` | Replace inline `<header>` with `{% include "partials/dm_header.html" %}`. |
| Edit | `server/templates/partials/sidebar.html` | Apply `text-slate-400` when `peer.mute_mode != "none"`; pass `mute_mode` to `unread_badge.html`. |
| Add | `server/src/routes/dm_mute.rs` | `POST /dm/:peer_id/mute` handler. |
| Edit | `server/src/routes/dm.rs` | Pass `mute_mode` into `DmPage` so the toggle renders the right state. |
| Edit | `server/src/routes/mod.rs` | Register `mod dm_mute;` and the new route. |
| Add | `server/tests/db_dm_mute.rs` | DB tests for `set_dm_mute` + the `set_room_mute_mode` strict guard. |
| Add | `server/tests/routes_dm_mute.rs` | Route + WS-filter integration tests. |

## Tasks

### Task 1 - DB helpers + symmetric strict guards

- [ ] Edit `server/src/db/notifications.rs`. Add a private helper that
      returns the room kind, used by both setters. Place it just above
      `set_room_mute_mode`:

```rust
/// Internal helper: assert the room exists and matches the expected
/// kind (DM or not-DM). Returns `sqlx::Error::Protocol` on mismatch so
/// callers can surface a 400 at the route layer when needed; the same
/// signature works for both setters.
async fn assert_room_kind(
    pool: &SqlitePool,
    room_id: i64,
    expect_dm: bool,
) -> Result<(), sqlx::Error> {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT room_type FROM rooms WHERE id = ?",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    let kind = kind.ok_or_else(|| {
        sqlx::Error::Protocol(format!("room {room_id} not found").into())
    })?;
    let is_dm = kind == "dm";
    if is_dm != expect_dm {
        let want = if expect_dm { "dm" } else { "non-dm" };
        return Err(sqlx::Error::Protocol(
            format!("room {room_id} is '{kind}', expected {want}").into(),
        ));
    }
    Ok(())
}
```

- [ ] Tighten `set_room_mute_mode` to call `assert_room_kind(pool,
      room_id, false)` as its first step (before the `MuteMode::None`
      delete shortcut). New body:

```rust
pub async fn set_room_mute_mode(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    mode: MuteMode,
) -> Result<(), sqlx::Error> {
    assert_room_kind(pool, room_id, false).await?;
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
```

- [ ] Add `set_dm_mute` below `delete_room_mute_setting`:

```rust
/// Toggle the mute state for a DM room. `muted = true` writes
/// `MuteMode::All`; `false` deletes the row (absence = unmuted).
/// Rejects non-DM rooms via `assert_room_kind`.
pub async fn set_dm_mute(
    pool: &SqlitePool,
    user_id: &str,
    dm_room_id: i64,
    muted: bool,
) -> Result<(), sqlx::Error> {
    assert_room_kind(pool, dm_room_id, true).await?;
    if muted {
        sqlx::query(
            "INSERT INTO room_notification_settings \
                 (user_id, room_id, mute_mode, updated_at) \
             VALUES (?, ?, 'all', datetime('now')) \
             ON CONFLICT(user_id, room_id) DO UPDATE SET \
                 mute_mode = 'all', \
                 updated_at = datetime('now')",
        )
        .bind(user_id)
        .bind(dm_room_id)
        .execute(pool)
        .await?;
        Ok(())
    } else {
        delete_room_mute_setting(pool, user_id, dm_room_id).await
    }
}
```

      Note: this intentionally does NOT route through
      `set_room_mute_mode`, because that helper now rejects DM rooms.
      Both helpers do their own UPSERT against the same table.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git checkout -b feat/dm-mute`
- [ ] `git add server/src/db/notifications.rs`

### Task 2 - DB tests for `set_dm_mute` + strict guards

- [ ] Create `server/tests/db_dm_mute.rs`. Setup mirrors
      `db_notifications.rs` from phase 15 - in-memory SQLite chat pool
      with the full chat-migrations list, plus a helper for seeding a
      DM room (two `room_members` rows, `room_type = 'dm'`).

```rust
use lets_chat::db::notifications::{
    self, room_mute_mode, set_dm_mute, set_room_mute_mode, MuteMode,
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
        include_str!("../migrations/chat/0012_uploads.sql"),
        include_str!("../migrations/chat/0013_link_previews.sql"),
        include_str!("../migrations/chat/0014_mentions.sql"),
        include_str!("../migrations/chat/0015_room_notification_settings.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn seed_room(pool: &SqlitePool, name: &str, kind: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, ?)")
        .bind(name)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn set_dm_mute_writes_all_mode() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
}

#[tokio::test]
async fn set_dm_mute_false_deletes_row() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_dm_mute(&pool, "user-1", dm, false).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind("user-1")
    .bind(dm)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn set_dm_mute_idempotent_when_already_muted() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
}

#[tokio::test]
async fn set_dm_mute_rejects_non_dm_room() {
    let pool = setup_chat_pool().await;
    let public = seed_room(&pool, "general", "public").await;
    let res = set_dm_mute(&pool, "user-1", public, true).await;
    assert!(res.is_err(), "expected error, got {:?}", res);
}

#[tokio::test]
async fn set_dm_mute_rejects_missing_room() {
    let pool = setup_chat_pool().await;
    let res = set_dm_mute(&pool, "user-1", 9_999, true).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn set_room_mute_mode_rejects_dm_room() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    let res = set_room_mute_mode(&pool, "user-1", dm, MuteMode::All).await;
    assert!(res.is_err(), "expected DM rejection, got {:?}", res);
    let res = set_room_mute_mode(&pool, "user-1", dm, MuteMode::ExceptMentions).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn set_room_mute_mode_rejects_missing_room() {
    let pool = setup_chat_pool().await;
    let res = set_room_mute_mode(&pool, "user-1", 9_999, MuteMode::All).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn dm_mute_per_direction_independent() {
    // A muting their DM with B does not affect B's setting for the same
    // DM room (they are independent rows in the (user_id, room_id) table).
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-a", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-a", dm).await.unwrap(),
        MuteMode::All
    );
    assert_eq!(
        room_mute_mode(&pool, "user-b", dm).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn dm_mute_does_not_collide_with_room_mute_for_other_room() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    let r = seed_room(&pool, "general", "public").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
}
```

- [ ] `./dev/cargo test -p lets-chat-server --test db_dm_mute`
- [ ] `git add server/tests/db_dm_mute.rs`

### Task 3 - `SidebarPeer::mute_mode` plumbing

- [ ] Edit `server/src/views/layout.rs`. Add `pub mute_mode: String,`
      to `SidebarPeer`:

```rust
pub struct SidebarPeer {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub unread: i64,
    pub status: String,
    pub custom_status: Option<String>,
    /// "none" | "all". DMs only ever take these two; mirrors
    /// `SidebarRoom::mute_mode` so the sidebar template can apply the
    /// same greyed-link treatment uniformly.
    pub mute_mode: String,
    pub active: bool,
}
```

- [ ] Edit `server/src/routes/mod.rs::load_sidebar`. The DM branch
      currently does not call `room_mute_modes_for_user`; bulk-load it
      once and look up by DM `room.id`. Add just before the `for (room,
      peer_id) in &dm_rooms` loop:

```rust
let dm_mute_modes: HashMap<i64, db::notifications::MuteMode> =
    db::notifications::room_mute_modes_for_user(&state.chat, &user.id).await?;
```

      And in the `SidebarPeer { ... }` literal inside the loop, set
      `mute_mode`:

```rust
mute_mode: dm_mute_modes
    .get(&room.id)
    .copied()
    .unwrap_or(db::notifications::MuteMode::None)
    .as_str()
    .to_string(),
```

      Audit other `SidebarPeer { ... }` construction sites:

```bash
grep -rn "SidebarPeer {" server/src/
```

      As of phase 16 there is only the one in
      `routes/mod.rs::load_sidebar`. If a new construction surfaces
      during this audit (e.g. a test fixture), set
      `mute_mode: "none".to_string()`.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/views/layout.rs server/src/routes/mod.rs`

### Task 4 - Remove the three DM-bypass guards + add `DmMuteChanged`

- [ ] Edit `server/src/push/mod.rs::dispatch`. Replace the
      `if kind != "dm" { ... room_mute_mode ... }` block with an
      unconditional lookup:

```rust
let mode = db::notifications::room_mute_mode(&state.chat, recipient_user_id, *room_id)
    .await
    .unwrap_or(MuteMode::None);
if matches!(mode, MuteMode::All) {
    return;
}
```

      Drop the `// FUTURE: when the DM-mute phase lands ...` comment
      lines.

- [ ] Edit `server/src/routes/ws.rs::ws_handler` send loop. In the
      `Mentioned` arm (server/src/routes/ws.rs:131-158), remove the
      `if kind == "dm" { true } else { ... }` short-circuit. New body:

```rust
ChatEvent::Mentioned {
    mentioned_user_id,
    room_id,
    ..
} if mentioned_user_id == &send_user.id => {
    use crate::db::notifications::{room_mute_mode, MuteMode};
    match room_mute_mode(&send_state.chat, &send_user.id, *room_id).await {
        Ok(MuteMode::All) => None,
        _ => render_mentioned(&e),
    }
}
```

      Note: `MuteMode::ExceptMentions` falls through to
      `render_mentioned` because `_` matches it. For DM rooms,
      `ExceptMentions` is unreachable via the API (the helper rejects
      it), but DB-corrupt rows are handled gracefully - the worst case
      is a notification that shouldn't have fired, not a crash.

      The existing comments referencing "DM mute is a separate phase"
      go away.

- [ ] Edit `server/src/routes/room.rs`. In the DM branch of
      `post_message` (server/src/routes/room.rs:362-367), replace the
      `// FUTURE: when the DM-mute phase lands ...` lines with one
      sentence noting that mute is honored downstream:

```rust
state.hub.broadcast_to_user(&peer_id, &event);
// Mute is enforced downstream: the WS Mentioned arm and push::dispatch
// both consult `room_mute_mode(peer_id, dm_room_id)` and drop the
// event when the peer has muted this DM.
crate::push::dispatch(&state, &peer_id, &event).await;
```

- [ ] Edit `server/src/ws/events.rs`. Add to `ChatEvent`:

```rust
/// Per-user notification of a DM-mute toggle. Recipients re-render
/// their sidebar OOB so the muted-peer class flips and badges
/// hide/show across all of their open tabs. Routed via
/// `Hub::broadcast_to_user(user_id, ...)`.
DmMuteChanged {
    /// The room id of the affected DM (the canonical key for the
    /// (muter, dm_room_id) row in `room_notification_settings`).
    dm_room_id: i64,
    /// The peer of the affected DM. Carried for clarity; the sidebar
    /// re-render does not consult it directly.
    peer_user_id: String,
    muted: bool,
},
```

- [ ] Edit `server/src/views/ws_fragments.rs::render_event`. Add
      `DmMuteChanged` to the not-rendered match arms list (it is
      rendered per-recipient in `routes/ws.rs`, mirroring
      `RoomNotifyPrefsChanged`).

- [ ] Edit `server/src/routes/ws.rs`. In the send loop's `match &e`
      block, add a `DmMuteChanged` arm next to the existing
      `RoomNotifyPrefsChanged` arm:

```rust
ChatEvent::DmMuteChanged { .. } => {
    // The event is only routed via `broadcast_to_user(muter_id, ...)`,
    // so reaching this arm already implies the recipient is the muter.
    // Re-render the sidebar OOB so the peer row's greyed-link class
    // and unread-badge visibility flip in this tab.
    render_sidebar(&send_state, &send_user).await
}
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/push/mod.rs server/src/routes/ws.rs server/src/routes/room.rs server/src/ws/events.rs server/src/views/ws_fragments.rs`

### Task 5 - Sidebar template: greyed muted peers + suppressed badge

- [ ] Edit `server/templates/partials/sidebar.html`. In the peers
      `<a>` opening tag, add the `else if peer.mute_mode != "none"`
      class branch (mirrors the rooms section):

      Replace:

```html
<a href="/dm/{{ peer.id }}"{% if peer.active %} aria-current="page"{% endif %} class="flex items-center gap-2 px-2 py-1 rounded hover:bg-slate-200 focus:bg-slate-200 focus:outline-none focus:ring-2 focus:ring-blue-500{% if peer.active %} bg-blue-100 font-semibold text-blue-900{% endif %}">
```

      with:

```html
<a href="/dm/{{ peer.id }}"{% if peer.active %} aria-current="page"{% endif %} class="flex items-center gap-2 px-2 py-1 rounded hover:bg-slate-200 focus:bg-slate-200 focus:outline-none focus:ring-2 focus:ring-blue-500{% if peer.active %} bg-blue-100 font-semibold text-blue-900{% else if peer.mute_mode != "none" %} text-slate-400{% endif %}">
```

- [ ] In the same loop, replace the hardcoded `{% let mute_mode =
      "none" %}` line with the peer's actual mute mode. Old:

```html
{% let mute_mode = "none" %}
```

      New:

```html
{% let mute_mode = peer.mute_mode.as_str() %}
```

      The included `partials/unread_badge.html` already suppresses the
      badge when `mute_mode != "none"` (phase 15 wiring). No change to
      `unread_badge.html` is needed.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/templates/partials/sidebar.html`

### Task 6 - DM header partial + mute toggle template

- [ ] Create `server/templates/partials/dm_header.html`. Wrapper id
      `lc-dm-header` is the swap target the POST handler returns.
      The mute toggle is a single `<label>` with a `<input
      type="checkbox" hx-post>`; HTMX swaps the whole header on change.

```html
<header id="lc-dm-header" class="border-b border-slate-200 px-4 py-2 flex items-center gap-3">
  {% let avatar_user_id = peer.id.clone() %}
  {% let avatar_username = peer.username.clone() %}
  {% let avatar_ext = peer.avatar_ext.clone() %}
  {% let avatar_status = peer.status.clone() %}
  {% let avatar_custom_status = peer.custom_status.clone() %}
  {% let avatar_size = "h-8 w-8 text-sm" %}
  {% include "partials/avatar.html" %}
  <div class="flex-1 min-w-0">
    <h1 class="font-semibold truncate">{{ peer.display_label() }}</h1>
    <div class="text-xs text-slate-500 truncate">@{{ peer.username }}</div>
    <div data-user-custom="{{ peer.id }}" class="text-xs italic text-slate-500 truncate">{% if let Some(t) = peer.custom_status.as_ref() %}{{ t }}{% endif %}</div>
  </div>
  <label class="inline-flex items-center gap-1 text-xs text-slate-600 cursor-pointer select-none" title="Mute notifications from this DM">
    <input type="checkbox"
           name="muted"
           hx-post="/dm/{{ peer.id }}/mute"
           hx-trigger="change"
           hx-target="#lc-dm-header"
           hx-swap="outerHTML"
           {% if mute_mode == "all" %}checked{% endif %}
           class="h-3 w-3">
    Mute
  </label>
  <form method="post" action="/users/{{ peer.id }}/block" onsubmit="return confirm('Block @{{ peer.username }}? They won\'t be able to message you and you won\'t see their messages.');">
    <input type="hidden" name="return_to" value="/">
    <button type="submit" class="text-xs text-slate-500 hover:text-red-600 hover:underline">Block</button>
  </form>
</header>
```

- [ ] Edit `server/templates/dm/page.html`. Replace the inline
      `<header>` block with the partial include. The new `main` block
      reads:

```html
{% block main %}
<div class="flex flex-1 overflow-hidden">
  <div class="flex flex-col flex-1 overflow-hidden">
    {% include "partials/dm_header.html" %}
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

- [ ] Add `mute_mode: String` to `DmPage` in `server/src/views/dm.rs`.
      The struct gains one field; no other field changes.

- [ ] Create `server/src/views/dm_header.rs`:

```rust
use askama::Template;

use crate::models::{Room, User};

#[derive(Template)]
#[template(path = "partials/dm_header.html")]
pub struct DmHeaderFragment<'a> {
    pub peer: &'a User,
    pub room: &'a Room,
    pub mute_mode: &'a str,
}
```

- [ ] Add `pub mod dm_header;` to `server/src/views/mod.rs`.

- [ ] Edit `server/src/routes/dm.rs::get_dm`. Just before the `DmPage`
      literal, look up the viewer's mute mode for this DM room:

```rust
let mute_mode = db::notifications::room_mute_mode(&state.chat, &user.id, room.id)
    .await
    .unwrap_or(db::notifications::MuteMode::None)
    .as_str()
    .to_string();
```

      And pass it into `DmPage`:

```rust
let page = DmPage {
    user: &user,
    peer: &peer,
    room: &room,
    sidebar_rooms: &sidebar_rooms,
    sidebar_peers: &sidebar_peers,
    switcher: &switcher,
    messages: &messages,
    asset_version: &state.asset_version,
    mute_mode: mute_mode.clone(),
};
```

      Match the existing borrowing convention of `DmPage` (the
      surrounding fields use both `&'a User` and owned `String`s; the
      `String` form is fine here).

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/templates/partials/dm_header.html server/templates/dm/page.html server/src/views/dm.rs server/src/views/dm_header.rs server/src/views/mod.rs server/src/routes/dm.rs`

### Task 7 - `POST /dm/:peer_id/mute` route

- [ ] Create `server/src/routes/dm_mute.rs`:

```rust
use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::dm_header::DmHeaderFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Deserialize, Default)]
pub struct DmMuteForm {
    /// Standard HTML checkbox semantics: present (any value) when checked,
    /// absent when unchecked. Serde's `Option<String>` covers both.
    pub muted: Option<String>,
}

/// POST /dm/:peer_id/mute
///
/// Toggle the viewer's mute setting for their DM with `peer_id`. Returns
/// the swapped `#lc-dm-header` fragment so the requesting tab updates
/// inline. Other open tabs of the same user receive a `DmMuteChanged`
/// event over WS and re-render their sidebar.
///
/// Block interaction: blocked DMs render as `WelcomePage` instead of the
/// DM page (`routes/dm.rs::get_dm`), so the toggle is unreachable from
/// the UI. The handler does not duplicate the block check - mute is a
/// private per-user setting and muting an already-existing DM is
/// harmless even when the conversation is gated.
pub async fn post_dm_mute(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(peer_id): Path<String>,
    Form(form): Form<DmMuteForm>,
) -> Result<Html, AppError> {
    if peer_id == user.id {
        return Err(AppError::BadRequest("cannot mute a DM with yourself".into()));
    }

    let peer_record = db::auth::find_user_by_id(&state.auth, &peer_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let peer: User = peer_record.into();

    let room = db::chat::find_dm_room(&state.chat, &user.id, &peer.id)
        .await?
        .ok_or(AppError::NotFound)?;

    if !db::chat::is_room_member(&state.chat, room.id, &user.id).await? {
        return Err(AppError::Forbidden);
    }

    let muted = form.muted.is_some();
    db::notifications::set_dm_mute(&state.chat, &user.id, room.id, muted).await?;

    let event = ChatEvent::DmMuteChanged {
        dm_room_id: room.id,
        peer_user_id: peer.id.clone(),
        muted,
    };
    state.hub.broadcast_to_user(&user.id, &event);

    let fragment = DmHeaderFragment {
        peer: &peer,
        room: &room,
        mute_mode: if muted { "all" } else { "none" },
    };
    html(&fragment)
}
```

- [ ] Edit `server/src/routes/mod.rs`. Add `mod dm_mute;` near the
      other route module declarations and register the route in
      `build_router`. Place it next to the existing
      `.route("/dm/{peer_id}", get(dm::get_dm))`:

```rust
.route("/dm/{peer_id}/mute", post(dm_mute::post_dm_mute))
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/routes/dm_mute.rs server/src/routes/mod.rs`

### Task 8 - Route + WS-filter integration tests

- [ ] Create `server/tests/routes_dm_mute.rs`. Setup follows the
      pattern in `server/tests/routes_mentions.rs`: build a router with
      two seeded users (viewer + peer), open chat/auth/settings pools
      with the full migration list, and exercise the `POST
      /dm/:peer_id/mute` handler plus Hub-level / dispatch-level
      assertions confirming muted DMs suppress events.

  Tests to include:

  1. `post_persists_dm_mute_and_returns_swapped_header`:
     - POST with `muted=on` to `/dm/{peer_id}/mute` (DM room
       pre-created via `find_or_create_dm_room`).
     - Assert response body contains `id="lc-dm-header"` and the
       `<input type="checkbox" name="muted" ... checked>` is present.
     - Assert `room_notification_settings` has a row with
       `mute_mode='all'` for `(viewer.id, dm_room.id)`.
  2. `post_with_muted_absent_deletes_row`:
     - Set the DM to muted, then POST with no `muted` form field.
     - Assert no row in `room_notification_settings` for
       `(viewer.id, dm_room.id)`.
  3. `post_unauthenticated_returns_redirect_or_401`:
     - POST without a session cookie. Assert the existing
       `AuthUser` rejection (mirrors other route tests).
  4. `post_with_unknown_peer_returns_404`:
     - POST `/dm/does-not-exist/mute`. Assert 404.
  5. `post_with_no_dm_room_returns_404`:
     - Create the peer user but do not seed a DM room. Assert 404.
  6. `post_self_dm_returns_400`:
     - POST `/dm/{viewer.id}/mute`. Assert 400.
  7. `dm_mute_all_suppresses_mention_render`:
     - Set `mute_mode=all` for viewer in DM room (via direct
       `set_dm_mute` call). Wire a `Hub`-bound test consumer for
       viewer's user-id channel. Have peer post a DM message in
       the DM room. Assert `render_mentioned` returns `None` when
       the WS arm is exercised directly with the resulting
       `Mentioned { kind: "dm", room_id: dm_room.id, ... }` event.
  8. `dm_unmuted_passes_mention_through`:
     - Same setup, no mute row. Peer posts a DM message. Assert
       `render_mentioned` returns `Some(html)`.
  9. `dm_mute_all_skips_push_dispatch`:
     - Use a `MockPushClient` injected into `AppState`. Subscribe
       a Push subscription for the viewer. Set DM mute. Have peer
       post a DM. Assert `MockPushClient::sent` is empty.
  10. `dm_unmuted_dispatches_push`:
      - Same setup, no mute. Assert `MockPushClient::sent` has
        exactly one entry addressed to viewer.
  11. `dm_mute_per_direction_independent`:
      - Viewer mutes DM with peer. Send a DM peer-to-viewer:
        suppressed. Send a DM viewer-to-peer (different
        direction): peer's render path is NOT filtered (peer has
        no mute row).
  12. `dm_mute_does_not_zero_unread_watermark`:
      - Set DM mute. Peer sends 3 DM messages. Assert
        `last_message_id - last_read_message_id == 3` for the
        viewer (i.e. the unread accumulates silently). Then GET
        `/dm/{peer_id}` as viewer; assert the watermark advances
        to the latest message id (mute is forward-looking; opens
        clear).
  13. `mark_mentions_read_still_runs_for_muted_dm_open`:
      - Although DM-kind events don't write mention rows
        (phase 14), the room-mute test (phase 15) verified the
        mark-read loop still fires for muted rooms when the user
        opens them. Verify the analogous claim for DMs: opening a
        muted DM still calls `set_last_read` and broadcasts a
        `DmRead` event.
  14. `sidebar_renders_muted_dm_with_greyed_class`:
      - Set DM mute. GET `/` (or any page that renders the
        sidebar with the DM peer visible). Assert the response
        HTML contains `text-slate-400` adjacent to
        `href="/dm/{peer_id}"`.
  15. `sidebar_unread_badge_hidden_for_muted_dm`:
      - Have peer post 3 DM messages so the viewer accumulates
        unread. Set DM mute. GET the sidebar page. Assert the
        response contains `<span id="unread-dm-{peer_id}"></span>`
        (empty span) rather than the badge with the count.
  16. `dm_mute_changed_event_re_renders_sidebar_for_other_tabs`:
      - Spawn a fake WS receiver for the viewer's user channel.
        POST `/dm/{peer_id}/mute` with `muted=on`. Assert the
        receiver gets a sidebar OOB-swap fragment that contains
        the greyed peer row.

- [ ] `./dev/cargo test -p lets-chat-server --test routes_dm_mute`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/tests/routes_dm_mute.rs`

### Task 9 - Final verification

- [ ] `just check-server`
- [ ] `just check-server-saas` (mute is mode-agnostic; should compile
      in both modes since none of the touched files are gated by a
      feature flag).
- [ ] `just check-clippy`
- [ ] `just check-clippy-saas`
- [ ] `just check-fmt` (run `./dev/cargo fmt --all` if it complains).
- [ ] `just test`
- [ ] `just test-saas`
- [ ] `just verify`

- [ ] Manual smoke-test list (`just dev-web-local`, log in as two
      users in two browsers / windows):

  1. As user A, open the DM with B. Confirm the header shows the
     "Mute" checkbox unchecked, the avatar/name/Block button
     remain in place.
  2. Tick the Mute checkbox. Confirm: the request fires, the
     header re-renders with the box checked, and (in another tab
     also logged in as A) the sidebar peer row for B turns grey
     and any unread DM badge disappears.
  3. As B, send a DM to A: "test 1". Confirm A sees no title
     flash, no favicon dot, no sound, no browser notification, no
     unread badge bump in the DM sidebar entry. If A has Push
     subscribed, no Push notification arrives on A's device.
  4. As A, open the DM with B. Confirm the message "test 1" is
     visible and the watermark clears (refresh: no unread badge
     would have been shown anyway, but the underlying state is
     now zero).
  5. Untick the Mute checkbox. Send another DM from B. Confirm A
     receives all notification surfaces normally.
  6. (Cross-tab consistency.) Open a third A-tab. Tick mute in
     tab 1. Confirm tabs 2 and 3 reflect the muted-grey sidebar
     entry without a refresh.
  7. (Room mute regression.) Open a regular room. Confirm the
     three-radio dropdown still works and that POST
     `/room/{id}/notify-prefs` with the dropdown still updates
     normally.
  8. (Block-DM regression.) Block B from the DM page. Confirm the
     DM page becomes the WelcomePage with the block message and
     the mute toggle is no longer rendered. Unblock; confirm the
     toggle returns and shows the persisted muted state.

## Things to confirm

- **`DmMuteChanged` vs reusing `RoomNotifyPrefsChanged`.** The user
  decided to introduce a new variant. Reasoning: a DM mute change
  needs a different render path, so reusing would force an `if
  room.room_type == "dm"` branch in the WS handler.

  In writing the WS render arm I notice the branch never actually
  appears: both events route to `render_sidebar(&send_state,
  &send_user)` unchanged. The DM mute is keyed by `dm_room_id` (which
  is just a `room_id`) and the sidebar renders all peers from
  `load_sidebar`'s output, picking up `mute_mode` from
  `room_mute_modes_for_user(user_id)` via the same `room_id` key. So
  the WS handler for `DmMuteChanged` is literally a copy of the
  `RoomNotifyPrefsChanged` arm with a different match pattern.

  Concrete trade-off:
  - **As-instructed (`DmMuteChanged`):** one extra `ChatEvent`
    variant, one extra match arm, one extra `render_event` no-op
    entry. The two arms are byte-identical save for the pattern.
  - **Reuse `RoomNotifyPrefsChanged`:** zero new variant. The
    `room_id` field already exists; we'd just send `room_id =
    dm_room_id` and `mute_mode = "all" | "none"`. The handler is
    untouched.

  The user explicitly said "if reuse is genuinely shape-agnostic
  push back briefly and I'll decide. Don't silently reverse it." The
  shape is genuinely agnostic. Flagging here as instructed; the plan
  is written for `DmMuteChanged` as the user's stated default. If you
  want to switch to reuse, the only changes are: drop
  `ChatEvent::DmMuteChanged`, drop the new WS arm, and have
  `routes/dm_mute.rs` emit `RoomNotifyPrefsChanged { user_id,
  room_id: dm_room.id, mute_mode: if muted { "all" } else { "none" } }`
  instead.

- **`assert_room_kind` returns `sqlx::Error::Protocol`.** This keeps
  the `Result<(), sqlx::Error>` signature and avoids cascading a new
  error type through callers. The cost is that the route layer
  receives a generic `sqlx::Error` and would surface as 500 if the
  helper guard fires (which only happens on DB-corrupt or hand-crafted
  bad input, not real users). The route layer does its own validation
  before the helper call (`room.room_type` check), so the helper
  guard is defense-in-depth and a 500 in this path is acceptable. If
  you'd rather see a richer `db::notifications::Error` enum with
  `From<NotificationsError> for AppError`, the change is mechanical -
  flag here for review.

- **No `notify_prefs.rs` change.** Phase 16 already added a `if
  room.room_type == "dm"` rejection in
  `routes/notify_prefs.rs::post_notify_prefs`. That stays exactly as
  is - it remains the regression guard so a future template bug
  can't route DM mute changes through the room endpoint.

- **`muted_until` column up-front (carried over from phase 15).**
  Still unused, still reserved for the future time-bound-mute phase.
  No code in this phase reads or writes it.

- **Bulk-loading mute modes once per page render.** `load_sidebar`
  already calls `room_mute_modes_for_user` for the rooms branch; this
  phase adds the same call to the DM branch. If the call site
  becomes the bottleneck for huge rosters, the future fix is to
  call it once per request and share the map across both branches.
  Not a phase-17 concern.
