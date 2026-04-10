-- Add room_type and created_by to rooms
ALTER TABLE rooms ADD COLUMN room_type TEXT NOT NULL DEFAULT 'public';
ALTER TABLE rooms ADD COLUMN created_by TEXT;

-- Room members table for DM participant tracking
CREATE TABLE IF NOT EXISTS room_members (
    room_id   INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id   TEXT NOT NULL,
    joined_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (room_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_room_members_user ON room_members(user_id);
