-- LC-64: per-room message drafts. A row holds the in-progress textarea
-- contents for one (user, room) pair so opening the room on another
-- device (or after a refresh) restores the half-typed message.
--
-- PK on (user_id, room_id): a user has at most one draft per room. UPSERT
-- is last-write-wins on `updated_at`; no operational-transform shape -
-- drafts are a single user's private scratch space, the only realistic
-- conflict is "I typed on my laptop, then typed on my phone," and the
-- newer write wins. Anything fancier would be wildly disproportionate
-- to private unsent text.
--
-- room_id ON DELETE CASCADE: when a room is deleted, every draft in it
-- vanishes via the FK. No explicit cleanup needed in delete_room.
--
-- No FK from user_id to anything: auth users live in a separate pool
-- (auth.db), matching the convention everywhere else in chat.db. The
-- account-delete path (routes/account.rs::purge_user_chat) handles the
-- per-user cleanup transactionally; same pattern as scheduled_messages.
--
-- No FK from (user_id, room_id) to room_members: room membership lives
-- in chat.db's room_members table for DMs / private rooms, but public
-- rooms have no membership row. Joining against room_members for a
-- per-user constraint would only cover the private-room subset; not
-- worth the inconsistency. Visibility checks happen at the PUT handler
-- and at render time, not at the schema level.
--
-- updated_at is the SQLite "YYYY-MM-DD HH:MM:SS" UTC shape, matching
-- every other datetime column in chat.db, so `updated_at < datetime
-- ('now', '-60 days')` is a plain string comparison.
CREATE TABLE IF NOT EXISTS message_drafts (
    user_id    TEXT    NOT NULL,
    room_id    INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    body       TEXT    NOT NULL,
    updated_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, room_id)
);

-- Per-user cleanup path: purge_user_chat deletes WHERE user_id = ?, and
-- the eventual room-leaves a user does also DELETE WHERE user_id = ? AND
-- room_id = ?. The (user_id, room_id) PK already provides the second
-- shape's index. This secondary index covers the first.
CREATE INDEX IF NOT EXISTS idx_message_drafts_user
    ON message_drafts (user_id);
