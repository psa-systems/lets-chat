-- LC-339: "Coyote Mode" per-enclave anti-spam.
--
-- When enabled on an enclave, a member who posts in 3+ distinct rooms of the
-- enclave within <=3 seconds is treated as a bot: banned from the enclave
-- (kick + ban-list below, so they cannot rejoin) and their last-24h messages
-- in the enclave's rooms are soft-deleted. Default off; enclave managers
-- toggle it alongside the LC-217 rate limit.
ALTER TABLE enclaves ADD COLUMN coyote_mode INTEGER NOT NULL DEFAULT 0;

-- Enclave-scoped ban-list. No enclave-level ban existed before (only kick via
-- enclave_members removal, which a public enclave lets the user immediately
-- undo by rejoining). A row here blocks rejoin and posting in that enclave.
CREATE TABLE enclave_bans (
    enclave_id INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    user_id    TEXT    NOT NULL,
    reason     TEXT,
    banned_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (enclave_id, user_id)
);

-- Keeps the per-insert burst-detection query (COUNT(DISTINCT room_id) by author
-- in a 3s window) cheap. IF NOT EXISTS guards against a pre-existing index.
CREATE INDEX IF NOT EXISTS idx_messages_user_created ON messages(user_id, created_at);
