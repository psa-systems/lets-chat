# Message Grouping Design

## Summary

When a user sends multiple messages in close succession with no other sender in between, render the run as one visual group: only the first message shows the username and timestamp header. Follow-up messages render with body, reactions, and seen indicator only, with tighter vertical spacing.

## Goals

- Reduce visual noise from repeated username/timestamp lines on bursty senders.
- Match common chat UX (Slack-style grouping).
- Preserve full per-message functionality: edit, delete, react, seen receipt.

## Non-Goals

- No client-side grouping logic. Server renders the grouping decision into the HTML.
- No retroactive regrouping when messages are reordered (messages are append-only by `created_at`).
- No per-user or per-room configuration of the grouping threshold.

## Grouping Rule

A message is a "follow-up" of its immediately-prior message in the same thread when:

1. Same `user_id`.
2. `current.created_at - prior.created_at <= 5 minutes` (inclusive at the boundary).

Otherwise it is a "header" message. The first message in a thread is always a header.

The 5-minute threshold is a fixed constant. Not configurable.

## Architecture

### Data Model

Add `is_follow_up: bool` to `MessageView` in `server/src/views/room.rs`. Computed at render time, not stored in the database. The `messages` table is unchanged.

Define a shared constant:

```rust
// server/src/db/chat.rs (or new shared module)
pub const MESSAGE_GROUPING_WINDOW: chrono::Duration = chrono::Duration::minutes(5);
```

### Initial Page Load

Loaders that build a list of `MessageView` (room page, DM page) compute follow-up flags in a single chronological pass:

```rust
let mut prev: Option<(&str, DateTime<Utc>)> = None;
for msg in messages_chrono {
    let is_follow_up = match prev {
        Some((pu, pt)) => pu == msg.user_id && (msg.created_at - pt) <= MESSAGE_GROUPING_WINDOW,
        None => false,
    };
    prev = Some((&msg.user_id, msg.created_at));
    // build MessageView with is_follow_up
}
```

### New Message (POST + WS broadcast)

The POST handler in `server/src/routes/room.rs` and `server/src/routes/dm.rs`:

1. Insert the new message.
2. Query the immediately-prior message in the thread (single SQL: `ORDER BY created_at DESC LIMIT 1 OFFSET 1` after insert, or query before insert by `created_at < now`).
3. Compute `is_follow_up`.
4. Render `ws/new_message.html` and broadcast.

### Delete Message (Promote-on-Delete)

When a header message is deleted, the next message in the thread (if it was a follow-up of the deleted message) becomes orphaned. Promote it.

Algorithm in the DELETE handler:

1. Load the target message (need `user_id`, `created_at`, `thread_id`).
2. Query the next message in the thread by `created_at ASC` where `created_at > target.created_at LIMIT 1`.
3. Delete the target.
4. If next exists AND `next.user_id == target.user_id` AND `next.created_at - target.created_at <= MESSAGE_GROUPING_WINDOW`: the next message was a follow-up of the deleted message and is now orphaned.
   - Re-render `next` as a `MessageView` with `is_follow_up = false`.
   - Broadcast OOB `outerHTML` swap targeting `#msg-{next.id}`.
5. Broadcast the existing deletion fragment for the target.

Edit handler is unchanged. Edits do not change `user_id` or `created_at`, so grouping is invariant under edit.

### Template Changes

`server/templates/room/message.html`:

- Wrap the header `<div class="flex items-baseline gap-2">` in `{% if !message.is_follow_up %}...{% endif %}`.
- Move edit/delete buttons out of the header into an absolute-positioned overlay so they remain hover-revealed on follow-ups:

  ```html
  {% if message.can_edit || message.can_delete %}
  <span class="absolute right-2 top-1 opacity-0 group-hover:opacity-100 flex gap-2 text-xs">
    ...edit/delete buttons...
  </span>
  {% endif %}
  ```

  The outer `<div>` already has `class="... group"`. Add `relative` so the overlay positions correctly.

- Reduce padding on follow-ups: `{% if message.is_follow_up %}py-0.5{% else %}py-2{% endif %}`.

Reactions, seen indicator, and the body remain rendered for every message.

## Components

| File | Change |
|---|---|
| `server/src/views/room.rs` | Add `is_follow_up: bool` to `MessageView` |
| `server/src/db/chat.rs` | Add `MESSAGE_GROUPING_WINDOW` constant; add `compute_follow_up` helper; update message loaders to compute flags; add `next_message_after(thread, created_at)` helper |
| `server/src/routes/room.rs` | POST handler queries prior, computes flag. DELETE handler promotes next if needed. |
| `server/src/routes/dm.rs` | Same as room.rs but for DM threads. |
| `server/templates/room/message.html` | Conditionally render header; relocate edit/delete to hover overlay; conditional padding. |

WebSocket hub and fragment templates (`ws/new_message.html`, `ws/deleted_message.html`) need no structural change. The promote-on-delete broadcast reuses the single-message OOB pattern already used for `edited_message.html`.

## Data Flow

### POST message

```
HTTP POST /rooms/{id}/messages
  -> insert into chat.db
  -> query prior message in thread
  -> compute is_follow_up
  -> render ws/new_message.html
  -> hub.broadcast(thread_id, fragment)
```

### DELETE message (header case)

```
HTTP DELETE /messages/{id}
  -> load target (user_id, created_at, thread_id)
  -> query next message in thread
  -> delete target row
  -> if next was a follow-up of target:
       render MessageView(next, is_follow_up=false) as ws/edited_message.html
       hub.broadcast(thread_id, promote_fragment)
  -> hub.broadcast(thread_id, delete_fragment)
```

### Initial load

```
GET /rooms/{id}
  -> load N most recent messages chronologically
  -> single pass: compute is_follow_up using running prev pointer
  -> render room/page.html
```

## Error Handling

- Prior-message query returns no row (first in thread): `is_follow_up = false`. No error.
- Next-message query on delete returns no row: skip promote. No error.
- DB error on either query: bubble through existing `AppError`. Same handling as any other DB failure in those handlers.
- No special handling for clock skew. `created_at` is server-side SQLite `CURRENT_TIMESTAMP`.

## Testing

### Unit tests in `server/tests/`

- `test_grouping_consecutive_same_user_within_window` - 3 messages from user A, 1s apart, first is header, rest are follow-ups.
- `test_grouping_breaks_on_different_user` - A, B, A: all 3 are headers.
- `test_grouping_breaks_on_time_gap` - A at t=0, A at t=6min: both headers.
- `test_grouping_at_exact_threshold` - 5min boundary inclusive (`<=`).
- `test_promote_on_delete_header` - A1, A2, A3 grouped; delete A1; A2 becomes header; A3 still follow-up.
- `test_delete_follow_up_no_promote` - delete A2 (follow-up); A3 still follow-up of A1.
- `test_delete_lone_message` - single message in thread; delete; no promote-side effect.

### Integration tests

- `test_room_page_renders_grouping` - POST 3 messages from same user; GET room page; assert only first contains the username header markup.
- `test_dm_page_renders_grouping` - same for a DM thread.
- `test_promote_on_delete_renders_header` - POST 2 grouped messages; DELETE first; GET page; second message now contains username header markup.

All tests use the existing in-memory SQLite pool harness in `server/tests/`.

## Out of Scope

- Avatar rendering on header messages (no avatars exist in the app today).
- Cross-thread grouping (messages are scoped to their room or DM thread).
- Configurable grouping window.
- Time-of-day "session breaks" beyond the simple 5-minute rule.
