-- Soft-delete columns for messages
ALTER TABLE messages ADD COLUMN deleted_at TEXT;
ALTER TABLE messages ADD COLUMN deleted_by TEXT;

-- Moderation audit log
CREATE TABLE IF NOT EXISTS mod_actions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    action      TEXT NOT NULL,
    target_user TEXT NOT NULL,
    actor_user  TEXT NOT NULL,
    reason      TEXT,
    room_id     INTEGER,
    metadata    TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_mod_actions_created_at ON mod_actions(created_at);
CREATE INDEX IF NOT EXISTS idx_mod_actions_target ON mod_actions(target_user);
