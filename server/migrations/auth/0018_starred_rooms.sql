-- LC-80: per-user starred / favorited rooms. Star state is private (each
-- user maintains their own list and ordering), so the table lives in
-- auth.db alongside other per-user preferences. `room_id` references a
-- chat.db `rooms.id`; cross-db FK is not enforceable in SQLite, so the
-- route layer scrubs orphan rows on room-leave (mirror of LC-79's
-- `forget_rooms` pattern).
CREATE TABLE IF NOT EXISTS starred_rooms (
    user_id TEXT NOT NULL,
    room_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    starred_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, room_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_starred_rooms_user_position
    ON starred_rooms(user_id, position);
