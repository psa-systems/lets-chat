-- Phase 26: per-message edit history. Each row captures the body that was
-- displaced by an edit, plus the timestamp of the edit that displaced it.
-- The current body lives in messages.body; rendering history reads N rows
-- here in (edited_at, id) order and appends the current body as the tail.
--
-- editor_user_id is intentionally not stored: today PATCH /messages/:id
-- is author-only, so the editor is always (SELECT user_id FROM messages
-- WHERE id = message_edits.message_id). When admin/mod edit capability
-- lands, add the column and backfill existing rows with that subquery.

CREATE TABLE IF NOT EXISTS message_edits (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id      INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    previous_body   TEXT NOT NULL,
    edited_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_edits_message_id
    ON message_edits (message_id, edited_at);
