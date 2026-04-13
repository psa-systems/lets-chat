CREATE TABLE IF NOT EXISTS dm_read_state (
    user_id              TEXT    NOT NULL,
    room_id              INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    last_read_message_id INTEGER NOT NULL,
    updated_at           TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, room_id)
);
CREATE INDEX IF NOT EXISTS idx_dm_read_state_room ON dm_read_state(room_id);
