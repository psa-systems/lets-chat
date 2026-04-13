# Read Receipts — Design

**Status:** Approved (brainstorm phase)
**Date:** 2026-04-13
**Scope:** DMs only

## Summary

Add read receipts to DM conversations. Track a single high-water mark per (user, DM room). Show a "Seen {HH:MM}" caption under the sender's latest message the recipient has read, and show unread-count badges next to DMs in the sidebar. A per-user symmetric toggle lets users opt out of both sending and seeing receipts.

## Goals

- Users can see when their DM partner has read their messages.
- Users can see at a glance which DMs have unread messages.
- Users can disable read receipts entirely (symmetric: if off, you neither send nor see them).

## Non-goals

- Receipts in public or private (non-DM) rooms.
- Per-message receipt state beyond what a high-water mark provides.
- Delivery receipts (only read is tracked).
- Scroll-based "seen" detection — opening the DM with the tab visible marks as read.

## Data model

### Migration `migrations/chat/0006_read_receipts.sql`

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

### Migration `migrations/auth/0002_read_receipts.sql`

```sql
ALTER TABLE users ADD COLUMN read_receipts_enabled INTEGER NOT NULL DEFAULT 1;
```

Both migrations are registered in the per-domain migration runner at startup.

### Model changes

- `User` / `UserRecord` (`src/models/user.rs`): add `read_receipts_enabled: bool`.

## Server functions

### `mark_dm_read(room_id: i64, message_id: i64) -> Result<(), ServerFnError>`

1. `require_auth()`.
2. Load room; require `room_type = 'dm'` and caller is a member (reuse `require_room_access`-style check).
3. Upsert `dm_read_state (user_id, room_id, last_read_message_id)` — on conflict, `last_read_message_id = MAX(excluded.last_read_message_id, dm_read_state.last_read_message_id)`. Monotonic; older ids are no-ops.
4. Look up the other DM participant via `room_members`.
5. If **both** caller and peer have `read_receipts_enabled = true`, broadcast `ChatEvent::DmRead { room_id, user_id: caller, last_read_message_id, read_at: now }` to the room.

### `set_read_receipts_enabled(enabled: bool) -> Result<(), ServerFnError>`

1. `require_auth()`.
2. `UPDATE users SET read_receipts_enabled = ? WHERE id = ?`.

### `list_dm_unread_counts() -> Result<Vec<DmUnread>, ServerFnError>`

Returns `Vec<DmUnread { room_id: i64, count: i64 }>` — one entry per DM the user is a member of, with count = messages in that room where `id > COALESCE(last_read, 0)` and `user_id != caller`.

### `get_dm_peer_read_state(room_id: i64) -> Result<Option<PeerReadState>, ServerFnError>`

Returns the peer's `last_read_message_id` and `updated_at` for this DM, if any. Used on DM-view mount to render the "Seen" label before any live `DmRead` event arrives. Gated on symmetric consent (returns `None` if either user has disabled receipts).

## WebSocket

New variant in `src/ws/events.rs`:

```rust
DmRead {
    room_id: i64,
    user_id: String,          // who read
    last_read_message_id: i64,
    read_at: String,          // datetime string, matches existing conventions
}
```

Broadcast only on symmetric consent (see above). Delivered to the DM room subscribers.

## Client behavior

### DM view (`src/components/dm_view.rs`)

**Local state:**
- `peer_last_read_id: Signal<Option<i64>>`
- `peer_read_at: Signal<Option<String>>`

**On mount (and when `room_id` changes):**
1. Fetch messages (existing).
2. Call `get_dm_peer_read_state(room_id)` → populate `peer_last_read_id` / `peer_read_at`.
3. If tab is visible and latest message is not from self, call `mark_dm_read(room_id, latest_msg_id)`.

**On new `ChatEvent::NewMessage` for this room:**
- If author is peer and tab is visible, call `mark_dm_read(room_id, msg.id)`.

**On `visibilitychange` → visible:**
- Call `mark_dm_read(room_id, latest_peer_msg_id)` if there's an unread peer message.

Implementation detail: use `web-sys::window().document().visibility_state()` and an `EventListener` on `visibilitychange`. Remove the listener on component unmount.

**On `ChatEvent::DmRead { room_id = this room, user_id = peer }`:**
- Update `peer_last_read_id` and `peer_read_at`.

**Rendering "Seen {HH:MM}":**
- Find the most recent own-authored message with `id <= peer_last_read_id`.
- Render a small muted caption (`text-xs text-gray-400`) directly below it: `"Seen {HH:MM}"` where `HH:MM` is local time parsed from `peer_read_at`.
- Only one label at a time.

### Sidebar (`src/components/sidebar.rs`)

- On mount and on reconnect: call `list_dm_unread_counts()`, store in a `Signal<HashMap<i64, i64>>`.
- On `ChatEvent::NewMessage` where `is_dm = true` and the user is not viewing that DM: increment that room's count.
- When the user opens a DM (route change to that DM): optimistically set count to 0.
- Render a small badge (count if ≥ 1, or a dot) next to each DM entry.

### Settings toggle

A checkbox "Send and receive read receipts" wired to `set_read_receipts_enabled`. Placement: a user settings surface. If no suitable surface exists, add a minimal user-menu dropdown in the sidebar header with this toggle. Final placement confirmed during implementation planning by inspecting `layout.rs` / `sidebar.rs`.

On toggle off → any in-flight "Seen" label clears (reset `peer_last_read_id` to `None`).

## Privacy semantics

- `read_receipts_enabled` defaults to `true`.
- `mark_dm_read` writes always (so the user's own unread badges stay accurate). Only the **broadcast** and **peer read-state query** are gated.
- Gate is symmetric and AND-ed: both participants must have receipts enabled for either to see the other's read state.

## Error handling

- `mark_dm_read` on a non-DM room → `ServerFnError::new("Not a DM")`.
- `mark_dm_read` with `message_id` from another room → rejected implicitly by the MAX upsert being meaningless; additionally, verify the message belongs to `room_id` with a single query before upsert.
- All DB errors bubble up as `ServerFnError::new(e.to_string())` matching existing conventions.

## Testing

New file `tests/db_read_receipts.rs` using in-memory pools:

- `mark_dm_read` upsert is monotonic (older `message_id` is a no-op).
- `mark_dm_read` rejects non-members.
- `mark_dm_read` rejects non-DM rooms.
- `list_dm_unread_counts` returns 0 when caught up.
- `list_dm_unread_counts` counts only peer messages with `id > last_read`.
- `set_read_receipts_enabled` round-trips.
- `get_dm_peer_read_state` returns `None` when either side has opted out.

No WASM/component tests (consistent with the rest of the repo).

## Build sequence

1. Migrations + startup wiring (`src/db/mod.rs`).
2. `User`/`UserRecord` model additions.
3. `db::chat` queries (`upsert_dm_read`, `get_dm_peer_read_state`, `list_dm_unread_counts`); `db::auth` toggle setter/reader.
4. Server fns + `ChatEvent::DmRead` + symmetric-consent broadcast.
5. `dm_view.rs` — visibility-gated mark-read, "Seen" label, `DmRead` handler.
6. `sidebar.rs` — unread badges.
7. Settings toggle UI.
8. Tests.

Each step compiles on its own.
