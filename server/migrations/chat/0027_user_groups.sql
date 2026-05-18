-- LC-83: per-enclave user groups for @group-name mentions.
-- A group lives inside one enclave; its name must be unique within
-- that enclave but may collide across enclaves. ON DELETE CASCADE
-- through enclaves so removing an enclave drops its groups.
CREATE TABLE IF NOT EXISTS user_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (enclave_id, name),
    FOREIGN KEY (enclave_id) REFERENCES enclaves(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_user_groups_enclave_name
    ON user_groups(enclave_id, name);

CREATE TABLE IF NOT EXISTS user_group_members (
    group_id INTEGER NOT NULL,
    user_id TEXT NOT NULL,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (group_id, user_id),
    FOREIGN KEY (group_id) REFERENCES user_groups(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_user_group_members_user
    ON user_group_members(user_id);
