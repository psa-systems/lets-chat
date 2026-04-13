# lets-chat: Next Development Phases

Prioritized implementation plan for Phases 7–13. Each phase builds on the previous; Phases 9 (Private Rooms) and 11 (Search) have hard dependencies noted inline.

---

## Phase 7 — Message Editing (Done)

**Goal:** Let users correct their own messages after sending. Baseline expectation for any chat app. Unblocks Phase 11 (search must index canonical body).

### DB Migration — `migrations/chat/0004_message_editing.sql`

```sql
ALTER TABLE messages ADD COLUMN edited_at TEXT;
```

`edited_at IS NULL` means never edited. Follows the existing `deleted_at` pattern.

### Server Functions

| Function | Auth guard | Notes |
|---|---|---|
| `edit_message(message_id: i64, new_body: String)` | `require_auth()` | Reject if `user_id != caller.id` AND caller is not Admin/Mod. Reject if `deleted_at IS NOT NULL`. Reject if empty after trim. Enforce `max_message_length` from settings. |

### WebSocket Events

Add to `ChatEvent` in `src/ws/events.rs`:

```rust
MessageEdited {
    message_id: i64,
    room_id: i64,
    new_body: String,
    edited_at: String,
}
```

Broadcast immediately after the DB write, same pattern as `send_message`.

### UI Components

- `room_view.rs` / `dm_view.rs`: show an "Edit" button on hover for messages owned by the current user. Swap body text for an inline `<textarea>` pre-populated with the current body, with Save/Cancel. On `ChatEvent::MessageEdited`, update the message in the local signal without a full refetch. Display a small `(edited)` label when `edited_at` is set.
- `Message` model: add `edited_at: Option<String>` field.

### Integration Tests

- `edit_message` succeeds for message owner.
- `edit_message` is rejected for a different user (not mod/admin).
- `edit_message` on a soft-deleted message returns an error.
- Empty body after trim is rejected.
- Body exceeding `max_message_length` is rejected.

### Risks / Gotchas

- `Message` is shared across WASM and server. Adding `edited_at: Option<String>` requires updating every `Message` construction site in `list_messages`, `send_message`, and `dm.rs`.
- The `list_messages` cross-pool lookup (chat + auth per message) is already an N+1. Not made worse here, but worth addressing before Phase 11.

---

## Phase 8 — Typing Indicators (Done)

**Goal:** Show "Alice is typing…" in real time. Pure WebSocket, zero DB writes — highest UX-to-effort ratio of any remaining feature.

### DB Migration

None.

### Server Functions

None. The entire feature lives in the WS layer.

### WebSocket Events

Add to `ClientControl`:

```rust
Typing { room_id: i64 }
```

Add to `ChatEvent`:

```rust
UserTyping {
    room_id: i64,
    user_id: String,
    username: String,
}
UserStoppedTyping {
    room_id: i64,
    user_id: String,
}
```

### Hub Changes (`src/ws/hub.rs`)

Add a typing-state map:

```rust
// DashMap<(RoomId, UserId), Instant>
typing: DashMap<(i64, String), std::time::Instant>
```

When a `Typing` control frame arrives:
1. Record `(room_id, user_id) → Instant::now()`.
2. Broadcast `UserTyping` to the room (excluding the sender's connection).
3. Spawn a `tokio::time::sleep(5s)` task; when it fires, if the entry is still older than 5 seconds, remove it and broadcast `UserStoppedTyping`.

### UI Components

- `use_websocket.rs`: forward `UserTyping`/`UserStoppedTyping` through the existing signal.
- `room_view.rs` / `dm_view.rs`: maintain a local `Signal<Vec<String>>` of currently-typing usernames. Show "Alice is typing…" / "Alice and Bob are typing…" below the message input. Debounce the `Typing` control frame send to once per second while the user is actively typing.

### Integration Tests

Typing indicators are hard to integration-test without a real WS client. A single unit test on the hub's eviction logic (insert a stale entry, call cleanup, assert it is removed) is sufficient.

### Risks / Gotchas

- **Tokio task leak**: the eviction sleep task must check whether the entry was refreshed before broadcasting `UserStoppedTyping`. Use the stored `Instant` as a generation marker.
- **Cross-connection user lookup**: the hub needs a `ConnId → (user_id, username)` map to populate `UserTyping` without hitting the DB. Add this now — Phase 11 and Phase 9 will also want it.

---

## Phase 9 — Private / Invite-Only Rooms (Done)

**Goal:** Rooms that only members can see or post in. Architecturally foundational — gates access for search (Phase 11) and file uploads (Phase 13). Build before those phases to avoid retrofitting access checks.

### DB Migration — `migrations/chat/0005_private_rooms.sql`

```sql
-- rooms already has room_type TEXT DEFAULT 'public'
-- room_members already exists (used for DMs) — reuse it for private rooms
ALTER TABLE rooms ADD COLUMN invite_code TEXT UNIQUE;
CREATE INDEX IF NOT EXISTS idx_rooms_invite_code ON rooms(invite_code);
```

`room_members` already has the right schema. Private rooms reuse it — membership is checked the same way as DM participation.

### Server Functions

| Function | Notes |
|---|---|
| `create_room(name, topic, room_type)` | Admin only. Generate `invite_code` (random token) if `room_type = 'private'`. |
| `join_room_by_invite(invite_code)` | Any authenticated user. Inserts into `room_members`. |
| `invite_user_to_room(room_id, username)` | Admin/Mod. Insert target user into `room_members`. |
| `leave_room(room_id)` | Self-service. Delete own `room_members` row. |

Modify existing functions:

- `list_rooms()`: for non-Admin callers, LEFT JOIN `room_members` and filter: `room_type = 'public' OR room_members.user_id = ?`.
- `list_messages(room_id)`: add `require_room_access(room_id, user_id)` guard.
- `send_message(room_id)`: same guard.

### WebSocket Events

```rust
RoomMemberAdded { room_id: i64, user_id: String }
RoomMemberRemoved { room_id: i64, user_id: String }
```

Broadcast `RoomMemberRemoved` on kick/leave so the sidebar updates without a reload.

### UI Components

- **Admin `rooms.rs`**: room creation form gains a `room_type` dropdown (`public` / `private`). For private rooms, show the invite link and an "Invite user" input.
- **Sidebar**: `list_rooms()` already drives the list — no structural change needed, the server-side filter handles visibility.
- **Join via invite link**: new route `/invite/:code` that calls `join_room_by_invite` then redirects to the room.

### Integration Tests

- Non-member cannot call `list_messages` on a private room.
- Member can call `list_messages` after `join_room_by_invite`.
- Invalid invite code is rejected.
- `list_rooms` for a non-member does not include the private room.
- Admin can invite a user directly by username.

### Risks / Gotchas

- **DM rooms are already `room_type = 'dm'`**: ensure `require_room_access()` handles all three types correctly in a single helper rather than duplicating conditions.
- **`list_rooms` N+1 avoided**: use a single LEFT JOIN, not per-room membership checks in application code.
- **Invite code rotation**: provide an admin action to regenerate `invite_code` without changing the room ID, so old links can be invalidated.

---

## Phase 10 — Emoji Reactions

**Goal:** React to messages with emoji. High UX value, self-contained schema, no hard dependency on earlier phases.

### DB Migration — `migrations/chat/0006_reactions.sql`

```sql
CREATE TABLE IF NOT EXISTS message_reactions (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    emoji       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, user_id, emoji)
);

CREATE INDEX IF NOT EXISTS idx_reactions_message ON message_reactions(message_id);
```

Composite PK enforces one reaction per (message, user, emoji). Store `emoji` as Unicode character(s), not a name string.

### Server Functions

| Function | Notes |
|---|---|
| `add_reaction(message_id: i64, emoji: String)` | Auth required. Validate `emoji` is 1–8 Unicode chars. Insert or ignore. |
| `remove_reaction(message_id: i64, emoji: String)` | Auth required. Delete own row only. |
| `list_reactions(message_id: i64)` | Returns `Vec<(emoji, count, reacted_by_me)>`. Grouped by emoji. |

Reactions are loaded lazily per message, not bundled into `list_messages`.

### WebSocket Events

```rust
ReactionAdded {
    message_id: i64,
    room_id: i64,
    emoji: String,
    user_id: String,
}
ReactionRemoved {
    message_id: i64,
    room_id: i64,
    emoji: String,
    user_id: String,
}
```

### UI Components

- Message rendering: reaction bar below message body showing `[emoji] count` buttons. Clicking a reaction already added calls `remove_reaction`; clicking a new one calls `add_reaction`.
- Emoji picker: a small fixed set (top 20–30 common emoji) as a hover popover. No third-party library — static Rust/HTML.
- On `ReactionAdded`/`ReactionRemoved`, update local reaction counts without a server refetch.

### Integration Tests

- Add and remove reaction; verify count changes.
- Duplicate reaction (same user, same emoji) is idempotent.
- Reaction on a soft-deleted message: decide policy (allow or reject) and enforce it.
- `list_reactions` includes `reacted_by_me: true` for the calling user.

### Risks / Gotchas

- Do not bundle reactions into `list_messages` — fetch separately on demand to keep message payload lean.
- Validate `emoji.chars().count() <= 8` and reject control characters.

---

## Phase 11 — Message Search

**Goal:** Full-text search across messages the user has access to. **Depends on Phase 9** (private room membership must gate search results).

### DB Migration — `migrations/chat/0007_search.sql`

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    body,
    content=messages,
    content_rowid=id
);

-- Populate from existing messages
INSERT INTO messages_fts(rowid, body) SELECT id, body FROM messages WHERE deleted_at IS NULL;

-- Keep in sync
CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;

CREATE TRIGGER messages_fts_delete AFTER UPDATE OF deleted_at ON messages
WHEN new.deleted_at IS NOT NULL BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE OF body ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
```

### Server Functions

```rust
search_messages(query: String, room_id: Option<i64>) -> Result<Vec<SearchResult>, ServerFnError>
```

- Validate `query` is non-empty, under 200 chars.
- For non-Admin callers, build a subquery of accessible room IDs (public ∪ member rooms) and filter results to those rooms.
- Return up to 50 results ordered by FTS5 `rank`, with `room_id`, `room_name`, `message_id`, body snippet, `author_name`, `created_at`.
- Escape FTS5 special characters in the input (`'`, `"`, `*`) before passing to `MATCH`.

### UI Components

- Search bar in the sidebar header or a global `Ctrl+K` modal.
- `SearchResults` component: result cards showing room name, author, timestamp, and a highlighted snippet. Clicking a result navigates to the room.

### Integration Tests

- Search returns results from accessible rooms only.
- Search does not return soft-deleted messages.
- Private room messages are excluded for non-members.
- FTS5 special characters in query are handled without panicking.
- Edited message body is re-indexed correctly (via `messages_fts_update` trigger).

### Risks / Gotchas

- **FTS5 and SQLx**: the query macro cannot introspect virtual table column types. Use `sqlx::query_as` with raw queries for all FTS5 operations.
- **Content table sync**: `content=messages` means FTS5 reads back from the real table. Hard-deletes would leave the index stale — since the app only soft-deletes, the update trigger above is the right path.
- **Access check**: build the accessible-rooms list as a SQL subquery joined into the search query, not as application-level filtering over all results.

---

## Phase 12 — Unread Counts & Read Receipts

**Goal:** Badge rooms and DMs with unread message counts. Last foundational UX piece — without it the sidebar is a flat list with no urgency signal.

### DB Migration — `migrations/chat/0008_read_receipts.sql`

```sql
CREATE TABLE IF NOT EXISTS room_last_read (
    room_id         INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL,
    last_message_id INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (room_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_last_read_user ON room_last_read(user_id);
```

Track last message ID seen (not a timestamp) to avoid clock skew.

### Server Functions

| Function | Notes |
|---|---|
| `mark_room_read(room_id: i64)` | Upsert with `MAX(messages.id)` for that room. |
| `get_unread_counts() -> Vec<(room_id, count)>` | Single query: `SELECT room_id, COUNT(*) FROM messages WHERE id > last_message_id AND deleted_at IS NULL GROUP BY room_id`. Returns only rooms where count > 0. |

### WebSocket Integration

On receiving `ChatEvent::NewMessage`, call `mark_room_read` only if the user is currently viewing that room. Otherwise increment a local unread counter. This avoids a server round-trip per message when the room is open.

### UI Components

- **Sidebar**: fetch `get_unread_counts()` on mount and on each `NewMessage` event for rooms not currently open. Render a count badge next to room/DM names.
- **Room/DM view**: call `mark_room_read` when the component mounts and when a new message arrives while the view is active.

### Integration Tests

- `get_unread_counts` returns 0 for a room just marked read.
- Sending a message increments the count for other users, not the sender.
- `mark_room_read` after receiving 5 messages sets count to 0.
- Soft-deleted messages are not counted as unread.

### Risks / Gotchas

- **Debounce `mark_room_read`**: if called from the WS event handler on every incoming message, it will fire a server function per message while the room is open. Debounce to once per 2–3 seconds or batch on component unmount.
- **New users joining a private room (Phase 9)**: initialize `room_last_read` row with `last_message_id = 0` in `join_room_by_invite` so `get_unread_counts` treats all messages as unread. The query must LEFT JOIN and handle NULL `last_message_id` as 0.

---

## Phase 13 — File & Image Uploads

**Goal:** Attach files and images to messages. Saved last — only phase with significant infrastructure risk due to the storage backend decision.

### Architectural Decision Required First

| Option | Pros | Cons |
|---|---|---|
| Local filesystem (`LETS_CHAT_DATA_DIR/uploads/`) | Zero dependencies, works in Docker with a volume | Breaks with multiple replicas |
| S3-compatible (MinIO, Tigris, R2) | Scales, presigned URLs offload bandwidth | Requires config and credentials |

**Recommendation**: local filesystem first (consistent with the SQLite-everything philosophy). Design `storage_path` in the DB so it can be swapped to an S3 URL later without schema changes.

### DB Migration — `migrations/chat/0009_file_uploads.sql`

```sql
CREATE TABLE IF NOT EXISTS file_uploads (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id   INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    uploader_id  TEXT NOT NULL,
    filename     TEXT NOT NULL,
    mime_type    TEXT NOT NULL,
    size_bytes   INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_uploads_message ON file_uploads(message_id);
```

### Backend Endpoints

File uploads cannot go through Dioxus server functions (JSON serialization). Add raw Axum multipart endpoints in `main.rs` alongside the existing `/ws` route:

```
POST /api/upload    (multipart/form-data → { file_id, url })
GET  /api/files/:id (streams file, auth-gated)
```

### Server Functions

`send_message_with_attachment(room_id, body, file_id)`: after the client uploads via `/api/upload`, it calls this server function with the returned `file_id` to create the message and link the upload.

### WebSocket Events

Add `attachments: Vec<Attachment>` to the `Message` model (empty `Vec` by default to preserve backward compatibility with existing `ChatEvent::NewMessage` consumers).

### Integration Tests

- Upload a valid image (PNG under limit) and retrieve it.
- Upload rejected if MIME type is not in the allowlist.
- Upload rejected if size exceeds `max_upload_bytes` setting (add to settings.db).
- File retrieval returns 403 for a user who cannot access the room the file was posted in.

### Risks / Gotchas

- **Multipart outside server functions**: `axum::extract::Multipart` does not compose with Dioxus's server function routing. The upload endpoint must be registered before `DioxusRouter` consumes the router.
- **MIME type validation**: never trust the `Content-Type` header. Use the `infer` crate to detect actual file magic bytes after receiving the upload. Define an explicit allowlist (e.g. image/jpeg, image/png, image/gif, image/webp, application/pdf).
- **Streaming large files**: do not buffer the entire upload in memory. Stream to disk with a size counter and abort once `max_upload_bytes` is exceeded.
- **Auth on file serving**: `/api/files/:id` must look up `file_id → message_id → room_id` and call `require_room_access()` from Phase 9. Without this, private room attachments are publicly accessible by URL guessing.
