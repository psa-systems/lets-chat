CREATE TABLE IF NOT EXISTS push_subscriptions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       TEXT    NOT NULL,
    endpoint      TEXT    NOT NULL UNIQUE,
    p256dh_key    TEXT    NOT NULL,
    auth_key      TEXT    NOT NULL,
    user_agent    TEXT,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_push_subscriptions_user
    ON push_subscriptions (user_id);

ALTER TABLE users ADD COLUMN notify_push_enabled INTEGER NOT NULL DEFAULT 0;
