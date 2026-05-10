# Phase 19 - Pinned Messages

## Goal

Let any user who can post in a room (or either party in a DM) pin a
message so it sticks to the top of the conversation as a header strip.
Pins survive scrolling, page reloads, browser tabs, and the lifetime
of the message itself; they vanish only when explicitly unpinned or
when the underlying message is deleted. The kind of feature users
reach for instinctively the first time they want to surface "the
agenda link," "the on-call rota," or "what we decided last Tuesday."

User-visible behaviour after this phase:

- Any room/DM message picks up a Pin / Unpin entry in its hover menu
  alongside the existing Reply / Edit / Delete buttons. No confirm
  dialog: pinning is reversible.
- Each room/DM page renders a header strip directly below the
  existing room/DM header showing the most recent pins (truncated to
  ~80 chars each) with author + relative timestamp. Each pin is a
  hash link to `#msg-{id}` so clicking scrolls the original message
  into view.
- A "See all (N) pinned" link in the strip navigates to `GET
  /room/:id/pins` (or `/dm/:id/pins`) - a standalone page listing
  every pin newest-first.
- Pin/unpin events fan out over the existing WS so other tabs and
  other room members re-render the strip live, same pattern as phases
  15/17.
- Hard cap of 50 pins per room. The 51st pin attempt returns 409 with
  a clear error.
- Soft-deleted messages disappear from the pinned view automatically;
  the pin row stays in the DB and reappears if a moderator ever
  un-deletes the message.

Out of scope (deferred):

- **Pin notifications.** Pinning is metadata, not communication. No
  `Mentioned` fan-out, no Push, no unread bump. The header-strip WS
  refresh is the only signal.
- **Pin reordering.** Pins always sort newest-first by `pinned_at`.
- **Pin expiration.** Pinned forever until explicitly unpinned.
- **Cross-room pins.** A pin belongs to its room; no global pin
  surface.
- **Per-pin permissions.** Anyone-who-can-pin can also unpin; no
  "only the original pinner can unpin" semantics.
- **Pin reactions / pin comments.**

## Architecture

- **Stack** (current truth): Axum 0.8 + Askama + HTMX, three SQLite
  pools (`auth`, `chat`, `settings`), pre-rendered HTML fragments
  over WebSocket. Pinned messages live entirely in the `chat` pool.

- **Schema, single new table.** Migration `0016_pinned_messages.sql`:

  ```sql
  CREATE TABLE pinned_messages (
      message_id    INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
      room_id       INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
      pinned_by     TEXT NOT NULL,
      pinned_at     TEXT NOT NULL DEFAULT (datetime('now'))
  );

  CREATE INDEX idx_pinned_messages_room ON pinned_messages(room_id, pinned_at DESC);
  ```

  `message_id` as PRIMARY KEY enforces "a message is pinned at most
  once" without a separate UNIQUE constraint. `room_id` denormalised
  so the per-room query does not have to join through `messages` and
  the index covers the only access pattern (newest-first per room).
  Cascade delete on both FKs: pinning is forward-looking metadata, so
  hard-deleting a message or room cleanly removes its pin rows.

- **Soft-delete interaction: filter at query time, do not delete the
  pin row.** The existing message model uses `deleted_at IS NULL` as
  the filter (verified across `server/src/db/chat.rs` lines 108, 138,
  168, 221, 251, 277, 689 and others - the pattern is consistent).
  Every pin-list query joins
  `pinned_messages JOIN messages ON id = message_id WHERE
  messages.deleted_at IS NULL`. Pin rows survive the soft-delete and
  reappear automatically if the message is restored, which costs us
  nothing. Option B (eagerly deleting pin rows on soft-delete) would
  require touching `delete_message` and adding asymmetric cleanup to
  any future soft-delete code path; not worth it.

- **Permissions: anyone who can post can pin.** Reuses the existing
  room-membership check at the route boundary (`db::chat::is_room_member`
  for private and DM rooms, public access for non-private rooms). For
  DMs both parties are room members by construction, so either party
  can pin and either party can unpin regardless of who originally
  pinned. DM mute does NOT affect pin visibility - both parties
  always see the pinned strip. No new `can_pin(user, room)` helper;
  inline the existing membership check at the two route handlers.

- **Limit: 50 pins per room, enforced at insert.** `pin_message`
  performs a `SELECT COUNT(*)` on the room's pins inside the same
  transaction as the INSERT and returns
  `sqlx::Error::Protocol("pin cap reached")` if the count is already
  50. The route handler maps that to `409 Conflict` with the body
  "Pin cap reached (50). Unpin a message first." Header-strip render
  shows top-3 inline; "See all (50) pinned" link reveals the rest.

- **UI surfaces:**
  - **Hover-menu Pin/Unpin button.** Added to
    `server/templates/room/message.html` lines 29-37 alongside Reply
    / Edit / Delete. Gated on `MessageView::is_pinned` (new field):
    pinned messages render "Unpin" with `hx-delete`, unpinned
    messages render "Pin" with `hx-post`. Both target
    `#msg-{{ message.id }}` for the bubble update; the route handler
    additionally returns the OOB-tagged pinned strip so the header
    refreshes in the same response. No `hx-confirm`.
  - **Header strip partial** at
    `server/templates/partials/pinned_strip.html`. Wrapper element
    `<div id="lc-pinned-strip-{{ room_id }}">` so the WS OOB swap
    target is unambiguous per room (matches the existing `lc-`
    prefixed-id convention in `partials/room_header.html`,
    `partials/dm_header.html`, `partials/connection_status.html`).
    Empty render (no DOM beyond the wrapper) when count is 0; top-3
    pins inline + "See all (N) pinned" link when count > 3.
  - **Header strip mount point.** Included in `server/templates/room/page.html`
    immediately after `{% include "partials/room_header.html" %}`
    and in `server/templates/dm/page.html` immediately after
    `{% include "partials/dm_header.html" %}`. Sits between the
    header and the message list so it does not push the composer out
    of view.
  - **Full pin list page.** `GET /room/:id/pins` and
    `GET /dm/:peer_id/pins` render `server/templates/room/pins.html`
    extending `layout.html`. Standalone page (not a modal) - simpler
    than building a drawer, and the back button is the user's escape
    hatch. Shows every pinned message in the room newest-first with
    the same rendering style as a normal message bubble plus an
    "Unpin" affordance.

- **WebSocket events.** Two new variants on `ChatEvent`:
  - `MessagePinned { room_id, message_id, pinned_by }`
  - `MessageUnpinned { room_id, message_id }`

  Both fan out via `hub.broadcast_to_room(room_id, &event)` so all
  room subscribers receive them. The render arm in
  `server/src/routes/ws.rs` re-renders the header strip for the
  receiving viewer's room context and emits the `lc-pinned-strip-{room_id}`
  OOB swap. We do NOT also emit a "pin badge" on the message bubble
  itself this phase: the strip update is the canonical signal, and
  changing the bubble outerHTML for a metadata change would invite
  scroll position weirdness in long rooms. If we want a pin icon on
  the bubble in a future phase, it can ride on top of the existing
  `EditedMessageFragment` re-render path.

- **Bulk-load pin state per page.** `get_room` and `get_dm` already
  load message lists; both will gain a single
  `db::pinned::pinned_message_ids_for_room(pool, room_id)` call
  returning `HashSet<i64>` and pass that down so each `MessageView`
  is built with `is_pinned: pinned_ids.contains(&m.id)`. One extra
  query per page render, no N+1. The same set drives the header
  strip render so we do not re-query.

- **Helper extraction: none.** The pin/unpin handlers consist of:
  membership check (existing), pin/unpin call, broadcast, return
  fragment. Same shape as the room-mute and DM-mute handlers from
  phases 15 and 17. No new abstraction warranted; if a third
  metadata-toggle endpoint shows up later we can consider a
  `metadata_toggle!` macro then. Phase 17 deferred a unified
  notification helper for the same reason - keep it inline.

## Tech Stack

- New crates: none.
- New static assets: none.
- New migrations: `server/migrations/chat/0016_pinned_messages.sql`.
- No new build steps; pure Rust + Askama + Tailwind classes already
  in the built stylesheet.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add  | `server/migrations/chat/0016_pinned_messages.sql` | Single table + index. |
| Edit | `server/src/db/mod.rs` | Add the new migration to the `chat` migration list. |
| Add  | `server/src/db/pinned.rs` | `pin_message`, `unpin_message`, `pins_for_room`, `count_for_room`, `pinned_message_ids_for_room`. |
| Edit | `server/src/db/mod.rs` | `pub mod pinned;`. |
| Edit | `server/src/ws/events.rs` | Add `ChatEvent::MessagePinned` and `ChatEvent::MessageUnpinned` variants. |
| Edit | `server/src/views/ws_fragments.rs` | Skip both new variants in `render_event` (handled inline like `RoomNotifyPrefsChanged` and `DmMuteChanged`). |
| Add  | `server/src/views/pinned.rs` | `PinnedStripFragment`, `PinnedListPage` Askama struct(s) + a per-row `PinnedRow` view type. |
| Edit | `server/src/views/mod.rs` | `pub mod pinned;`. |
| Edit | `server/src/views/room.rs` | Add `is_pinned: bool` to `MessageView`. |
| Edit | `server/src/views/dm.rs` | Same: thread `is_pinned` through the DM view if it duplicates `MessageView` shape, else this is a no-op (DMs reuse `MessageView`). |
| Add  | `server/src/routes/pinned.rs` | `POST /messages/:id/pin`, `DELETE /messages/:id/pin`, `GET /room/:id/pins`, `GET /dm/:peer_id/pins`. |
| Edit | `server/src/routes/mod.rs` | `pub mod pinned;` + register the four routes. |
| Edit | `server/src/routes/room.rs` | In `get_room`: bulk-load pinned ids, build the strip fragment, populate `MessageView::is_pinned`. |
| Edit | `server/src/routes/dm.rs` | Same as above for the DM page render. |
| Edit | `server/src/routes/ws.rs` | Add render arms for `MessagePinned` / `MessageUnpinned` that emit the OOB strip refresh. |
| Add  | `server/templates/partials/pinned_strip.html` | Header strip with top-3 + "See all (N)" link. Wrapped in `id="lc-pinned-strip-{{ room_id }}"`. |
| Add  | `server/templates/room/pins.html` | Full pin list page extending `layout.html`. |
| Edit | `server/templates/room/page.html` | Include the strip partial right after the room header. |
| Edit | `server/templates/dm/page.html` | Include the strip partial right after the DM header. |
| Edit | `server/templates/room/message.html` | Add Pin/Unpin button to the hover-menu cluster (lines 29-37 in current file), gated on `message.is_pinned`. |
| Add  | `server/tests/db_pinned.rs` | DB-level: pin / unpin / for_room respects soft-delete / cap enforcement / cascade delete on message + room. |
| Add  | `server/tests/routes_pinned.rs` | Route-level: 200 happy path room + DM, 403 non-member, 409 over cap, 404 nonexistent message, cross-room edge case (pinning a message that does not belong to the URL room id is rejected), DM either-party-can-unpin. |

## Tasks

> **Note on commits**: per the user's standing constraint for this
> phase, Claude stages with `git add` and stops. The user reviews and
> commits per task. Each task ends with a `git add ...` line and no
> `git commit`. Continue to the next task as soon as the previous
> task's check + stage are complete - do not wait for the commit to
> appear.

### Task 1 - Migration + DB layer

- [ ] Create `server/migrations/chat/0016_pinned_messages.sql` with
      the schema in the architecture section.

- [ ] Edit `server/src/db/mod.rs`. Add the new file to the chat
      migration `include_str!` list (mirror the pattern from migration
      0015).

- [ ] Create `server/src/db/pinned.rs` with the helpers:

      ```rust
      use std::collections::HashSet;
      use sqlx::SqlitePool;

      pub const MAX_PINS_PER_ROOM: i64 = 50;

      pub struct PinnedRow {
          pub message_id: i64,
          pub room_id: i64,
          pub pinned_by: String,
          pub pinned_at: String,
          pub author_user_id: String,
          pub author_username: String,
          pub author_display_name: Option<String>,
          pub body: String,
      }

      /// Insert a pin. Returns `Err(sqlx::Error::Protocol("pin cap reached"))`
      /// when the room already has `MAX_PINS_PER_ROOM` pins. The count +
      /// insert run inside a single transaction so two parallel pins on the
      /// 50th slot cannot both win.
      pub async fn pin_message(
          pool: &SqlitePool,
          message_id: i64,
          room_id: i64,
          pinned_by: &str,
      ) -> Result<(), sqlx::Error> { ... }

      pub async fn unpin_message(pool: &SqlitePool, message_id: i64)
          -> Result<(), sqlx::Error> { ... }

      /// Newest-first list of pinned, non-deleted messages in a room with
      /// author display fields joined from messages. `limit` lets the
      /// header strip ask for top-3 while the full-list page asks for
      /// the cap.
      pub async fn pins_for_room(
          pool: &SqlitePool,
          room_id: i64,
          limit: i64,
      ) -> Result<Vec<PinnedRow>, sqlx::Error> { ... }

      pub async fn count_for_room(pool: &SqlitePool, room_id: i64)
          -> Result<i64, sqlx::Error> { ... }

      /// Bulk lookup for the per-page render. Returns the set of message
      /// ids in the room that are currently pinned and whose underlying
      /// message is not soft-deleted.
      pub async fn pinned_message_ids_for_room(pool: &SqlitePool, room_id: i64)
          -> Result<HashSet<i64>, sqlx::Error> { ... }
      ```

      Implementation notes:
      - `pin_message` body: `BEGIN`, `SELECT COUNT(*) FROM
        pinned_messages WHERE room_id = ?`, compare to
        `MAX_PINS_PER_ROOM`, INSERT or return Protocol error,
        `COMMIT`. The PRIMARY KEY on `message_id` makes a duplicate
        pin a SQLite constraint violation; map that to a successful
        no-op (idempotent pin) rather than bubbling the error.
      - All read queries: `INNER JOIN messages m ON m.id = pm.message_id
        AND m.deleted_at IS NULL` (the auth-side username/display_name
        require an additional user lookup; do that as a second pass
        the same way `chat::messages_for_room` populates author
        metadata, or LEFT JOIN if cross-pool joins are not possible -
        verify by reading how the existing code resolves author
        display fields, since `auth.db` and `chat.db` are separate
        pools).
      - `count_for_room` filters soft-deleted in the same way so the
        "See all (N)" count matches what the user sees.

- [ ] Edit `server/src/db/mod.rs`: `pub mod pinned;`.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo check -p lets-chat-server --no-default-features --features saas`
- [ ] `git checkout -b feat/pinned-messages`
- [ ] `git add server/migrations/chat/0016_pinned_messages.sql server/src/db/pinned.rs server/src/db/mod.rs`

### Task 2 - DB tests

- [ ] Create `server/tests/db_pinned.rs`. Setup mirrors
      `server/tests/db_dm_mute.rs` (in-memory chat pool with the full
      migration list including the new 0016). Tests:

      1. `pin_then_unpin_round_trips` - `count_for_room` reflects the
         change.
      2. `pin_idempotent` - pinning the same message twice succeeds
         both times; count stays at 1.
      3. `pins_for_room_excludes_soft_deleted` - pin a message, mark
         its row `deleted_at = datetime('now')`, assert it does not
         appear in `pins_for_room` and `count_for_room` skips it.
      4. `pin_cap_enforced` - insert 50 pins, the 51st returns
         `sqlx::Error::Protocol`. Body of the error matches "pin cap
         reached" so the route layer can branch on it.
      5. `cascade_delete_on_message_hard_delete` - hard-delete a
         message row, assert its `pinned_messages` row is gone
         (cascade FK).
      6. `cascade_delete_on_room_delete` - delete a room, assert all
         its pinned rows are gone.
      7. `pinned_message_ids_for_room` returns the right set,
         excludes soft-deleted, returns an empty set when nothing is
         pinned.

- [ ] `./dev/cargo test -p lets-chat-server --test db_pinned -j 2`
- [ ] `git add server/tests/db_pinned.rs`

### Task 3 - WS event variants and skip in render_event

- [ ] Edit `server/src/ws/events.rs`. Add to the `ChatEvent` enum:

      ```rust
      MessagePinned {
          room_id: i64,
          message_id: i64,
          pinned_by: String,
      },
      MessageUnpinned {
          room_id: i64,
          message_id: i64,
      },
      ```

- [ ] Edit `server/src/views/ws_fragments.rs`. In `render_event`, add
      arms that return `None` for both new variants (matches the
      `RoomNotifyPrefsChanged` and `DmMuteChanged` pattern - the
      actual render is handled inline in `routes/ws.rs` so the
      receiving viewer's room context drives the strip render).

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/ws/events.rs server/src/views/ws_fragments.rs`

### Task 4 - View structs + strip + full-list templates

- [ ] Create `server/src/views/pinned.rs`:

      ```rust
      use askama::Template;
      use crate::db::pinned::PinnedRow;

      pub struct PinnedStripView<'a> {
          pub message_id: i64,
          pub author_label: &'a str,
          pub pinned_at: &'a str,
          pub snippet: String,
      }

      #[derive(Template)]
      #[template(path = "partials/pinned_strip.html")]
      pub struct PinnedStripFragment<'a> {
          pub room_id: i64,
          pub total_count: i64,
          pub pin_path: &'a str,            // "/room/123/pins" or "/dm/peer_id/pins"
          pub top_pins: Vec<PinnedStripView<'a>>,
      }

      #[derive(Template)]
      #[template(path = "room/pins.html")]
      pub struct PinnedListPage<'a> {
          pub user: &'a crate::models::User,
          pub room_label: String,
          pub back_path: String,            // "/room/123" or "/dm/peer_id"
          pub pins: Vec<PinnedRow>,
      }
      ```

      `snippet` is the body truncated to ~80 chars with an ellipsis,
      computed by the route handler before constructing the view (NOT
      in the template, to keep template logic minimal). `author_label`
      collapses display_name vs username with the same rule used
      everywhere else (display_name if Some, else username) - reuse
      the existing helper if there is one.

- [ ] Edit `server/src/views/mod.rs`: `pub mod pinned;`.

- [ ] Edit `server/src/views/room.rs` to add `pub is_pinned: bool` on
      `MessageView`. Default to `false` everywhere it is constructed
      today (find every `MessageView { ... }` literal and add the
      field). The render-side change is in Task 7.

- [ ] Edit `server/src/views/dm.rs` if it has its own message view
      type; otherwise no-op (DMs reuse `MessageView`).

- [ ] Create `server/templates/partials/pinned_strip.html`:

      ```html
      <div id="lc-pinned-strip-{{ room_id }}" class="border-b border-slate-200 bg-slate-50">
        {% if total_count == 0 %}
        {# Empty wrapper so the OOB swap target always exists. #}
        {% else %}
        <ul class="px-4 py-2 space-y-1 text-sm">
          {% for p in top_pins %}
          <li class="flex items-center gap-2">
            <span class="text-amber-600" aria-hidden="true">
              <svg class="h-3.5 w-3.5" viewBox="0 0 20 20" fill="currentColor">
                <path d="M10 1a1 1 0 011 1v3.586l3.207 3.207A1 1 0 0114 10.5H11v6.586a1 1 0 11-2 0V10.5H6a1 1 0 01-.707-1.707L8.5 5.586V2a1 1 0 011-1z"/>
              </svg>
            </span>
            <a href="#msg-{{ p.message_id }}" class="flex-1 truncate text-slate-700 hover:underline">
              <span class="font-medium">{{ p.author_label }}:</span>
              {{ p.snippet }}
            </a>
            <span class="text-xs text-slate-400 shrink-0">{{ p.pinned_at }}</span>
          </li>
          {% endfor %}
          {% if total_count > top_pins.len() as i64 %}
          <li>
            <a href="{{ pin_path }}" class="text-xs text-blue-600 hover:underline">See all ({{ total_count }}) pinned</a>
          </li>
          {% endif %}
        </ul>
        {% endif %}
      </div>
      ```

      The wrapper element renders unconditionally so the WS OOB swap
      always has something to replace. When `total_count == 0` the
      wrapper is empty (no padding, no border applied to children) -
      Tailwind's `empty:hidden` is not needed because the parent has
      no children to lay out.

- [ ] Create `server/templates/room/pins.html`:

      ```html
      {% extends "layout.html" %}

      {% block title %}Pinned in {{ room_label }} - lets-chat{% endblock %}

      {% block main %}
      <div class="flex flex-1 flex-col overflow-hidden">
        <header class="border-b border-slate-200 px-4 py-2 flex items-center justify-between gap-2">
          <h1 class="font-semibold truncate">Pinned in {{ room_label }}</h1>
          <a href="{{ back_path }}" class="text-sm text-blue-600 hover:underline">Back</a>
        </header>
        <div class="flex-1 overflow-y-auto">
          {% if pins.is_empty() %}
          <p class="p-4 text-sm text-slate-500">No pinned messages yet.</p>
          {% else %}
          <ul class="divide-y divide-slate-200">
            {% for p in pins %}
            <li class="px-4 py-3">
              <div class="flex items-center justify-between gap-2 text-xs text-slate-500">
                <span><span class="font-medium text-slate-700">{{ p.author_username }}</span> &middot; pinned by {{ p.pinned_by }} {{ p.pinned_at }}</span>
                <button hx-delete="/messages/{{ p.message_id }}/pin"
                        hx-target="closest li"
                        hx-swap="outerHTML"
                        class="text-red-600 hover:underline">Unpin</button>
              </div>
              <div class="mt-1 whitespace-pre-wrap text-sm">{{ p.body }}</div>
            </li>
            {% endfor %}
          </ul>
          {% endif %}
        </div>
      </div>
      {% endblock %}
      ```

      The Unpin button uses `hx-target="closest li"` + `hx-swap="outerHTML"`
      so the row vanishes on success; the route handler returns an
      empty body for that case and the WS event refreshes the strip
      on the room/DM page if the user has it open in another tab.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/views/pinned.rs server/src/views/mod.rs server/src/views/room.rs server/src/views/dm.rs server/templates/partials/pinned_strip.html server/templates/room/pins.html`

### Task 5 - Route handlers

- [ ] Create `server/src/routes/pinned.rs` with four handlers:

      1. `POST /messages/:id/pin` - resolve the message, look up its
         room, verify caller is allowed to post in that room
         (`is_room_member` for private/DM rooms, public for others).
         Call `db::pinned::pin_message`. On `sqlx::Error::Protocol("pin cap reached")`
         return 409 with the body `"Pin cap reached (50). Unpin a
         message first."` Broadcast `ChatEvent::MessagePinned`.
         Re-render the message bubble (with `is_pinned = true`) AND
         append the OOB-tagged strip fragment so the requesting tab
         updates both surfaces in one HTMX swap. Return the combined
         HTML.

      2. `DELETE /messages/:id/pin` - same shape, calls
         `db::pinned::unpin_message`, broadcasts
         `ChatEvent::MessageUnpinned`. Returns the message bubble +
         OOB strip fragment. The full-list page's per-row
         `hx-target="closest li"` works against this handler too: the
         response includes the OOB strip but the closest `<li>` swap
         removes the row from the page, so the OOB attribute simply
         finds no target on the pin-list page (htmx silently drops
         OOB swaps with no matching target) and is consumed only on
         the room/DM page.

      3. `GET /room/:id/pins` - resolve the room, verify caller can
         access it, fetch all pins via
         `db::pinned::pins_for_room(pool, room_id, MAX_PINS_PER_ROOM)`,
         render `PinnedListPage`.

      4. `GET /dm/:peer_id/pins` - same as above but resolves the DM
         room from the peer id (mirror what `routes/dm.rs::get_dm`
         does), then calls `pins_for_room`.

      Notes:
      - The "verify the URL room id matches the message's room"
        edge case lives only in the POST/DELETE handlers, since the
        URL is `/messages/:id/pin` and we get the `room_id` from the
        message itself. There is no cross-room URL surface to abuse.
      - DM mute does NOT short-circuit any of these handlers. Mute
        suppresses notifications, not data.
      - 404 cases: nonexistent message id, deleted message id (treat
        soft-deleted as 404 for pin/unpin), nonexistent room.
      - 403 cases: caller is not a member of a private room or DM.

- [ ] Edit `server/src/routes/mod.rs`: `pub mod pinned;` and register
      the four routes on the appropriate Axum router (mirror the
      pattern used for `routes::dm_mute`).

- [ ] Edit `server/src/routes/ws.rs`. Add render arms for
      `MessagePinned` and `MessageUnpinned` that fetch the strip
      fragment for the receiving viewer (`db::pinned::pins_for_room` +
      `count_for_room` + render `PinnedStripFragment`) and return the
      OOB-tagged HTML. The viewer-id check is unnecessary since the
      strip is room-scoped, not user-scoped: if you are subscribed to
      the room, the strip update applies to you.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo check -p lets-chat-server --no-default-features --features saas`
- [ ] `git add server/src/routes/pinned.rs server/src/routes/mod.rs server/src/routes/ws.rs`

### Task 6 - Bulk-load pin state and mount the strip

- [ ] Edit `server/src/routes/room.rs`. In `get_room`:
      1. After loading the message list, call
         `db::pinned::pinned_message_ids_for_room(&state.chat, room_id)`.
      2. Populate `MessageView::is_pinned` from the set when building
         each view.
      3. Build `PinnedStripFragment` with `total_count =
         db::pinned::count_for_room(...)` and `top_pins =
         db::pinned::pins_for_room(..., 3)` mapped through the
         truncation step (~80 chars).
      4. Pass the rendered strip into the `RoomPage` view so the
         template can include it just below the room header (or pass
         the data and let the template construct it - whichever is
         smaller).

- [ ] Edit `server/src/routes/dm.rs`. Same three steps for the DM
      page. The strip's `pin_path` is `"/dm/{peer_id}/pins"`; for
      rooms it is `"/room/{room_id}/pins"`.

- [ ] Edit `server/templates/room/page.html`. Right after
      `{% include "partials/room_header.html" %}`, add an include of
      the strip (or the rendered HTML field, depending on which
      direction the route handler took).

- [ ] Edit `server/templates/dm/page.html`. Same change after
      `{% include "partials/dm_header.html" %}`. The DM header
      partial currently lacks a `<div>` wrapper around the body, so
      the strip slots in cleanly between it and the messages list.

- [ ] Edit `server/templates/room/message.html`. Inside the
      hover-menu cluster (current lines 29-37), add immediately after
      the Reply button:

      ```html
      {% if message.is_pinned %}
      <button hx-delete="/messages/{{ message.id }}/pin"
              hx-target="#msg-{{ message.id }}"
              hx-swap="outerHTML"
              class="text-amber-700 hover:underline">Unpin</button>
      {% else %}
      <button hx-post="/messages/{{ message.id }}/pin"
              hx-target="#msg-{{ message.id }}"
              hx-swap="outerHTML"
              class="text-amber-700 hover:underline">Pin</button>
      {% endif %}
      ```

      Order in the cluster: Reply, Pin/Unpin, Edit (if can_edit),
      Delete (if can_delete). No `hx-confirm` on either pin button:
      both are reversible.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo check -p lets-chat-server --no-default-features --features saas`
- [ ] `git add server/src/routes/room.rs server/src/routes/dm.rs server/templates/room/page.html server/templates/dm/page.html server/templates/room/message.html`

### Task 7 - Route tests

- [ ] Create `server/tests/routes_pinned.rs`. Use the helper pattern
      from `server/tests/routes_dm_mute.rs` (see `app_with_two_users`
      and `seed_dm_room`). Tests:

      1. `pin_room_message_returns_200_and_strip_oob` - happy path
         in a room. Assert the response body contains the bubble
         (with the Unpin button this time) AND an OOB-tagged
         `lc-pinned-strip-{room_id}` element.
      2. `unpin_room_message_returns_200_and_strip_oob` - pin
         first, then unpin via `DELETE /messages/:id/pin`, assert the
         bubble flips back to the Pin button and the OOB strip is in
         the body.
      3. `pin_nonexistent_returns_404`.
      4. `pin_soft_deleted_returns_404` - mark a message
         `deleted_at = datetime('now')`, assert pinning it returns
         404 (not 200 then a phantom strip entry).
      5. `pin_in_unjoined_private_room_returns_403` - user posts the
         message in a public room, gets kicked into a private room
         they are not a member of, attempts to pin a message there.
      6. `pin_cap_returns_409` - seed 50 pins, the 51st returns 409
         with the documented body string.
      7. `dm_either_party_can_pin_and_unpin` - viewer pins peer's
         message; peer (other session) unpins via `DELETE
         /messages/:id/pin`. Both succeed.
      8. `get_room_pins_lists_all_in_order` - render `GET /room/:id/pins`,
         assert the response body has each pinned message body string
         in newest-first order.

- [ ] `./dev/cargo test -p lets-chat-server -j 2 --test routes_pinned`
- [ ] `git add server/tests/routes_pinned.rs`

### Task 8 - Final verification + manual smoke

- [ ] `just check`        # both modes + clippy + fmt
- [ ] `just test`         # standalone tests (with `-j 2` if the
                           # previous OOM during parallel linking
                           # recurs)
- [ ] `just test-saas`
- [ ] `just verify`       # build release + GET /login smoke

- [ ] **Manual smoke list** (run against `just dev-web-local`,
      with the browser DevTools Network tab open). Note results
      next to each item; report any deviations in the PR description:

      1. **Pin in a room.** Hover a message, click Pin. Bubble
         hover-menu now reads "Unpin"; header strip below the room
         header gains the entry.
      2. **Cross-tab pin propagation.** Open the same room in two
         tabs. Pin in tab A; assert tab B's header strip updates
         within ~1s without a refresh (WS OOB).
      3. **Unpin.** Click Unpin in the header strip's link target
         (jump to message via `#msg-{id}`, then unpin from the
         hover menu). Strip removes the entry; bubble button
         flips back to "Pin".
      4. **Cap.** Pin 50 distinct messages. The 51st attempt
         returns a visible error from the response body (htmx
         renders the 4xx body inline by default with the
         response-targets extension, so it should appear near
         the message). Confirm the body string matches "Pin cap
         reached (50). Unpin a message first."
      5. **Soft-delete cleanup.** Pin a message, then delete it
         via the existing Delete button. Header strip removes the
         entry on the next render (WS broadcast of the existing
         delete event re-renders the room list, which triggers
         the page's strip via the room-page render path - confirm
         the strip actually updates without a hard refresh; if
         not, log this as a follow-up to also broadcast a strip
         refresh on `MessageDeleted`).
      6. **Pinning in a DM.** Both parties can pin and unpin.
         Both see the strip live. DM mute does not affect this.
      7. **Full pin list page.** Click "See all (N) pinned" in
         the strip; lands on `/room/:id/pins` (or `/dm/.../pins`).
         Unpin a message from this page; the row is removed
         immediately and the underlying room/DM page reflects
         the change on next visit.
      8. **No new console errors.** No regressions in the
         existing notification, mute, or reconnect flows.

- [ ] **Hand back to user for commit + push.** Per the standing
      constraint for this phase, Claude does not commit or push.
      After Task 7's stage step the user reviews the staged diff,
      commits, and pushes when ready. If the user has been
      committing per task throughout, this final step is just
      `git status` to confirm a clean tree.

## Things that may surface during execution

These are not blockers; they are spots where reading the existing
code carefully matters more than usual.

1. **Cross-pool author display join.** `auth.db` and `chat.db` are
   separate SQLite pools (`AppState::auth` vs `AppState::chat`). A
   single SQL JOIN cannot span them. The two-pass pattern (fetch pin
   rows + message bodies from chat, then resolve usernames /
   display_name from auth in a second query) is what the existing
   message-list code does; mirror it. If `db::chat::messages_for_room`
   already populates a "MessageWithAuthor" struct via a helper, reuse
   that helper rather than re-rolling the auth-side lookup. Worth a
   targeted read of `server/src/db/chat.rs` (around the lines that
   already populate `author_name` / `display_name`) before writing
   `pins_for_room`.

2. **`MessageView::is_pinned` field-add scope.** Adding a field to
   `MessageView` ripples to every site that builds one literally.
   `server/src/routes/ws.rs` constructs `MessageView` in
   `render_new_message`, `render_edited_message`, and
   `render_thread_reply` (per the file we read for phase 18). All
   three need the field initialized - `false` is fine for new/edited
   messages since pin state is metadata that arrives via a separate
   event. The bulk-load only matters for the page render path.

3. **Strip element ID and OOB targeting.** Two open rooms in two
   tabs of the same browser, each subscribed to a different room,
   should see their own strip update independently. The
   `id="lc-pinned-strip-{{ room_id }}"` scheme handles this - the WS
   event carries the `room_id`, the OOB swap finds only the matching
   element. If the user has the same room open in two tabs, both
   tabs have the same id present and both swap. Verified by
   inspection of the existing `unread_badge.html` pattern, which
   uses the same id-by-room scheme.
