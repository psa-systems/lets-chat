-- LC-321: per-room nickname. A user's self-set display name scoped to a single
-- room, overriding their global display_name / username wherever their messages
-- render in that room. user_id lives in auth.db, so (matching room_members and
-- every other chat.db row that references a user) there is no cross-db FK on it;
-- room_id cascades so a deleted room drops its nicknames.
CREATE TABLE IF NOT EXISTS room_nicknames (
    room_id    INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL,
    nickname   TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (room_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_room_nicknames_user ON room_nicknames(user_id);
