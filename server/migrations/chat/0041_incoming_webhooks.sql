-- LC-74: incoming webhooks. An external system POSTs JSON to
-- /webhook/<secret> and it appears in a room as a message attributed to a
-- synthetic webhook actor (messages.user_id = '' + messages.webhook_id set).
--
-- Only an HMAC of the secret is stored (keyed by the server secret), so a
-- chat.db leak cannot reconstruct usable webhook URLs. The room FK cascades:
-- deleting a room removes its webhooks. Revoked rows are retained (revoked_at)
-- so later POSTs are refused without losing the audit trail.
ALTER TABLE messages ADD COLUMN webhook_id INTEGER;

CREATE TABLE IF NOT EXISTS incoming_webhooks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id      INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    avatar_url   TEXT,
    secret_hash  TEXT    NOT NULL UNIQUE,
    created_by   TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT,
    revoked_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_incoming_webhooks_room ON incoming_webhooks (room_id);
