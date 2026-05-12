ALTER TABLE users ADD COLUMN last_ws_seen_at TEXT;
ALTER TABLE users ADD COLUMN notify_email_digest_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN last_digest_sent_at TEXT;

CREATE INDEX IF NOT EXISTS idx_users_digest_eligible
    ON users (notify_email_digest_enabled, last_active_at)
    WHERE notify_email_digest_enabled = 1;
