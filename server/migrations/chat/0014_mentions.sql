CREATE TABLE IF NOT EXISTS mentions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id          INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    room_id             INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    mentioned_user_id   TEXT NOT NULL,
    author_user_id      TEXT NOT NULL,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    read_at             TEXT,
    UNIQUE (message_id, mentioned_user_id)
);

CREATE INDEX IF NOT EXISTS idx_mentions_unread
    ON mentions (mentioned_user_id, read_at);

CREATE INDEX IF NOT EXISTS idx_mentions_room_user
    ON mentions (room_id, mentioned_user_id);

CREATE INDEX IF NOT EXISTS idx_mentions_message
    ON mentions (message_id);
