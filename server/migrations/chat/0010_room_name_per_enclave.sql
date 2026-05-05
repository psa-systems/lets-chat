-- Drop the global UNIQUE constraint on rooms.name and replace with a
-- per-enclave partial unique index. SQLite has no DROP CONSTRAINT, so
-- recreate the table. Disable foreign keys during the rebuild so that
-- DROP TABLE does not cascade-delete dependent rows in messages /
-- room_members; the FK names resolve back to the renamed table.

PRAGMA foreign_keys = OFF;

CREATE TABLE rooms_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    topic       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    room_type   TEXT NOT NULL DEFAULT 'public',
    invite_code TEXT,
    created_by  TEXT,
    enclave_id  INTEGER REFERENCES enclaves(id) ON DELETE CASCADE
);

INSERT INTO rooms_new (id, name, topic, created_at, room_type, invite_code, created_by, enclave_id)
SELECT id, name, topic, created_at, room_type, invite_code, created_by, enclave_id FROM rooms;

DROP TABLE rooms;
ALTER TABLE rooms_new RENAME TO rooms;

CREATE UNIQUE INDEX idx_rooms_invite_code ON rooms(invite_code) WHERE invite_code IS NOT NULL;
CREATE INDEX idx_rooms_enclave ON rooms(enclave_id);
CREATE UNIQUE INDEX idx_rooms_name_per_enclave
    ON rooms(enclave_id, name) WHERE room_type != 'dm';

PRAGMA foreign_keys = ON;
