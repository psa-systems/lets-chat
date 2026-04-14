# Chat auto-scroll — design

**Date:** 2026-04-14
**Branch:** `feat/auto-scroll`

## Problem

Opening a chat (room or DM) leaves the viewport pinned at the very top of the message list, and new messages arriving over WebSocket never move the viewport. Users must manually scroll to the bottom every time, and they cannot tell at a glance where unseen messages start when returning to a busy chat.

## Goals

1. When a user opens a chat that has new messages since their last visit, scroll so the first unseen message is at the top of the viewport, with a visible divider above it.
2. When a user is actively reading at the bottom of a chat and a new message arrives, keep them pinned to the bottom (sticky-bottom).
3. When a user has scrolled up to read history and a new message arrives, do **not** yank the viewport — instead surface a "↓ New messages" pill they can click to jump down.
4. When a user opens a chat with no prior visit (or with nothing new since last visit), scroll to the bottom (the newest message).

## Non-goals

- Server-side unread tracking for rooms (DMs already have it; rooms will use a client-only heuristic).
- Sidebar unread badges for rooms.
- Deep links / jump-to-message.
- Changes to DM read-receipt semantics (`mark_dm_read`, `DmRead` event) — those continue to govern the "Seen" label on the peer side.

## Approach

### Last-seen tracking (client-only, uniform for rooms and DMs)

Store, per room, the highest message id the user has seen:

```
localStorage["lets-chat:last-seen:<room_id>"] = "<message_id>"
```

The DM server-side `last_read_message_id` is not reused for this purpose because:
- It does not exist for rooms, and we explicitly want to avoid server changes for this feature.
- It is updated as soon as a DM is opened (by `mark_dm_read`), which would erase the information needed to pick an initial scroll target.

Using a parallel client-only key keeps the two concerns independent: scroll positioning (this feature) vs. read-receipts (existing feature).

### Scroll target on chat open

On the first render where the message list for a room is non-empty:

1. Read `last_seen_on_open := localStorage["lets-chat:last-seen:<room_id>"]` (parse to `i64`, or `None`).
2. If `last_seen_on_open` is `Some(id)` and there is at least one message with `msg.id > id`:
   - Find the first such message.
   - `scrollIntoView({ block: "start" })` on that element.
   - Render an "↑ New messages" divider as a sibling immediately above that message.
3. Else:
   - Scroll to bottom (`container.scrollTop = container.scrollHeight`).

The open-time scroll happens once per chat-open. Switching rooms resets the state.

### Sticky-bottom on new-message arrival

When a WS `NewMessage` event arrives for the current room:

1. Before the DOM updates, sample `at_bottom = scrollTop + clientHeight >= scrollHeight - 50px`.
2. After the new message renders:
   - If `at_bottom` was true → `container.scrollTop = container.scrollHeight` (and update `last-seen` to the new newest id).
   - Else → set `show_new_messages_pill = true`.

The 50px slack absorbs sub-pixel rounding and the typing-indicator row that sits inside the scroll container boundary.

### "New messages" pill

When `show_new_messages_pill` is true, render a floating button pinned to the bottom-center of the scroll area (above the composer) with the text `↓ New messages`. Clicking it:
- Scrolls to bottom.
- Sets `show_new_messages_pill = false`.
- Updates `last-seen` to the newest message id.

The pill also auto-dismisses if the user manually scrolls to within 50px of the bottom.

### Updating `last-seen`

The stored last-seen id advances whenever the user is at (or scrolls to) the bottom:

- Sticky-bottom auto-scroll on new message → update to newest id.
- User clicks the new-messages pill → update to newest id.
- User manually scrolls to the bottom → update to newest id (detected via a `scroll` listener on the container).

It does **not** advance based on messages merely being rendered above the viewport. This preserves the "first unseen" behavior when the user leaves and returns without ever scrolling down.

## Component shape

New hook: `src/components/use_auto_scroll.rs`

```
pub struct AutoScroll {
    pub container_id: String,        // DOM id to attach to the scroll <div>
    pub show_new_pill: Signal<bool>,
    pub scroll_to_bottom: Callback<()>,
    pub first_unseen_id: Signal<Option<i64>>, // for rendering the divider
}

pub fn use_auto_scroll(room_id: Signal<i64>, messages: Signal<Vec<Message>>) -> AutoScroll;
```

Internally the hook:
- Generates a stable DOM id (`chat-scroll-<room_id>`) so it can look up the element via `document.getElementById`.
- Uses `use_effect` gated on `room_id` to capture `last_seen_on_open` and compute `first_unseen_id` from the first non-empty messages snapshot.
- Uses `use_effect` gated on `messages.len()` to run the sticky-bottom logic when a new message is appended.
- Attaches a `scroll` listener to the container (via `web_sys` inside a `use_effect`) to detect "user reached bottom" and update `last-seen` + dismiss the pill.

All `web_sys` / `localStorage` access is behind `#[cfg(target_arch = "wasm32")]`; on non-wasm targets the hook is a no-op returning default values.

## Integration points

1. `src/components/room_view.rs`
   - Call `use_auto_scroll(room_id_sig, messages)` after the existing `messages` signal is declared.
   - Add `id: "{auto.container_id}"` to the `<div class="flex-1 overflow-y-auto …">`.
   - Inside the `for msg in message_list.iter()` loop, render the "↑ New messages" divider above the message whose id equals `auto.first_unseen_id()`.
   - Render the new-messages pill conditionally.

2. `src/components/dm_view.rs`
   - Same integration. DM read-receipt logic (`mark_dm_read`, peer-read label) remains untouched.

3. No server-side changes. No migrations. No new server functions.

## Edge cases

- **Empty message list:** hook does nothing; no divider, no pill.
- **First visit ever to a room:** `last_seen_on_open` is `None` → scroll to bottom, no divider.
- **User sends a message:** their own outgoing message counts as "seen"; update `last-seen` to its id on send success.
- **Room switch:** `room_id_sig` changes → `first_unseen_id` recomputed on the first non-empty snapshot for the new room; pill dismissed.
- **Message deleted/edited:** no scroll change (length doesn't grow from deletes; edits don't append).
- **Visibility change (tab hidden):** no special handling for auto-scroll; sticky-bottom still fires when the tab is hidden because the DOM updates. This is consistent with current behavior.
- **localStorage unavailable (private mode, quota exceeded):** treat as "no prior visit" — scroll to bottom, skip writes; do not throw.

## Testing

- Manual verification in `just dev-web-local`:
  1. Fresh chat open with many messages → scrolled to bottom.
  2. Close chat, send messages from another session, reopen → first-unseen divider visible and scrolled into view at top.
  3. Scroll to bottom, receive a WS message → stays pinned to bottom.
  4. Scroll up, receive a WS message → pill appears, viewport does not move; click pill → jumps to bottom.
  5. Scroll up, then manually scroll back to bottom → pill auto-dismisses.
  6. Room switch → state resets; no stale pill or divider.

No automated tests are added — the behavior is DOM-scroll-driven and the project has no WASM/browser test harness.
