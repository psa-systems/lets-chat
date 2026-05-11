-- Per-user saved-messages list.
--
-- A user can save any message they can currently see (room or DM); the
-- bookmark row is private to that user. Bookmarks are looked up by user
-- when rendering the Saved page and by (user, message) when toggling
-- the Save/Unsave button on a message row.
CREATE TABLE IF NOT EXISTS bookmarks (
    user_id    TEXT    NOT NULL,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, message_id)
);

CREATE INDEX IF NOT EXISTS idx_bookmarks_user
    ON bookmarks (user_id, created_at DESC);
