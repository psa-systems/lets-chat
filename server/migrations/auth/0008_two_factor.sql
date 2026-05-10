ALTER TABLE users ADD COLUMN totp_secret_encrypted BLOB;
ALTER TABLE users ADD COLUMN totp_nonce BLOB;
ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN totp_recovery_hashes TEXT;

CREATE TABLE IF NOT EXISTS pending_2fa (
    token       TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_pending_2fa_user ON pending_2fa(user_id);
CREATE INDEX IF NOT EXISTS idx_pending_2fa_expires ON pending_2fa(expires_at);
