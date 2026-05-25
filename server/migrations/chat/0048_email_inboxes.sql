-- LC-77: per-room email-ingress inboxes. The email-shaped sibling of the
-- LC-74 incoming-webhooks table (chat/0041_incoming_webhooks.sql).
--
-- A room admin issues an inbox address (token@<ingress-domain>); only the
-- HMAC of the token is stored (keyed by the server secret, via
-- crate::auth::hash_api_token, the same hashing used for webhook secrets).
-- The plaintext token lives only in the address shown ONCE at creation.
--
-- Mail arriving at the polled mailbox addressed to a known token is posted
-- to the inbox's room as a synthetic "Email" actor (messages.user_id = '',
-- messages.email_inbox_id set, see chat/0049_messages_email_inbox_id.sql).
--
-- Mirrors LC-74 schema choices: ON DELETE CASCADE on room_id, soft-delete
-- via revoked_at, optional avatar_url, audit columns for created_by and
-- last_used_at.
CREATE TABLE IF NOT EXISTS email_inboxes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id       INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    avatar_url    TEXT,
    secret_hash   TEXT    NOT NULL UNIQUE,
    created_by    TEXT    NOT NULL,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now')),
    last_used_at  TEXT,
    revoked_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_email_inboxes_room ON email_inboxes (room_id);
