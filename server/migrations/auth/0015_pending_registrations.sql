CREATE TABLE IF NOT EXISTS pending_registrations (
    token                   TEXT PRIMARY KEY,
    username                TEXT NOT NULL,
    email                   TEXT,
    password_hash           TEXT NOT NULL,
    totp_secret_encrypted   BLOB NOT NULL,
    totp_nonce              BLOB NOT NULL,
    created_at              TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at              TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pending_registrations_expires ON pending_registrations(expires_at);
CREATE INDEX IF NOT EXISTS idx_pending_registrations_username ON pending_registrations(username);
