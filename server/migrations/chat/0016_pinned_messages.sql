CREATE TABLE IF NOT EXISTS pinned_messages (
    message_id    INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    room_id       INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    pinned_by     TEXT    NOT NULL,
    pinned_at     TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_pinned_messages_room
    ON pinned_messages (room_id, pinned_at DESC);
