ALTER TABLE users ADD COLUMN notify_login_alerts_enabled INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS login_alert_devices (
    user_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, fingerprint),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_login_alert_devices_user
    ON login_alert_devices(user_id);
