-- LC-487: per-user canned responses / saved replies.
--
-- Reusable message snippets a user invokes from the composer as a personal
-- slash command (`/name [args]`), expanded via the same static_text path as
-- admin custom commands (`{args}` is replaced with whatever the user typed
-- after the name, then posted as a normal message). Kept in a dedicated,
-- user-scoped table rather than `slash_commands_custom` because that table's
-- scope_id is an INTEGER and its UNIQUE(scope_kind, scope_id, name) would make
-- names globally unique across users; here uniqueness is per-user.
CREATE TABLE IF NOT EXISTS canned_responses (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     TEXT    NOT NULL,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    body        TEXT    NOT NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, name)
);

CREATE INDEX IF NOT EXISTS idx_canned_responses_user ON canned_responses(user_id);
