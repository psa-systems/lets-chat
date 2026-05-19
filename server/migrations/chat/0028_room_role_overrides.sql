-- LC-84: per-room role overrides. A row here elevates `user_id` to the
-- named role inside `room_id` only; the user's org-wide role (auth.db
-- users.role) still applies everywhere else. Storing only Moderator/
-- Admin keeps the override path elevation-only: removing an override
-- restores the org default. Per-room demotion is not supported.
--
-- ON DELETE CASCADE on `room_id`: when a room is deleted, its overrides
-- vanish with it (matches the room_members convention).
--
-- (room_id, user_id) PK forbids duplicate overrides for the same person
-- in the same room; the writer-side upsert replaces the role + audit
-- columns when re-granting.

CREATE TABLE IF NOT EXISTS room_role_overrides (
    room_id      INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id      TEXT    NOT NULL,
    role         TEXT    NOT NULL CHECK (role IN ('moderator', 'admin')),
    assigned_by  TEXT    NOT NULL,
    assigned_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (room_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_room_role_overrides_user
    ON room_role_overrides (user_id);
