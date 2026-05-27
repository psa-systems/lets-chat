-- LC-78: protocol bridge registration surface. Out-of-process daemons (Matrix,
-- IRC, XMPP) translate between lets-chat and a foreign protocol; the server's
-- job is just to track registered bridges, hold their sealed configuration,
-- and surface health from periodic heartbeats. Daemons authenticate to the
-- API as a dedicated bot user (LC-73) with bridge-scoped tokens (LC-72).
--
-- config_encrypted + config_nonce hold the AES-256-GCM-sealed daemon config
-- under LETS_CHAT_SECRET_KEY (same two-column convention as imap_inbox_config
-- in LC-77 and vapid_keys). A chat.db leak cannot reconstruct usable Matrix
-- shared secrets. The plaintext is an opaque-to-the-server config blob (JSON
-- in practice: homeserver URL, shared secret, etc.) shaped by the daemon.
--
-- kind is plain TEXT, not CHECK-constrained, so adding IRC / XMPP / future
-- protocols is a code-level validation loosening, not a migration. v1 accepts
-- 'matrix' only in the registration handler.
--
-- bot_user_id and created_by are TEXT without FKs because users live in
-- auth.db (separate SQLite database; cross-db FKs are not supported).
--
-- Removal semantics is STOP-NEW, not delete-history: the admin remove handler
-- DELETEs the bridges row, which triggers ON DELETE SET NULL on messages
-- (see chat/0055_messages_bridge_actor.sql) so the snapshot columns preserve
-- historical render. Chosen on principle: stop-new is reversible into
-- delete-history later via an additive branch, delete-history is not
-- reversible.
CREATE TABLE IF NOT EXISTS bridges (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id           INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    kind              TEXT    NOT NULL,
    config_encrypted  BLOB    NOT NULL,
    config_nonce      BLOB    NOT NULL,
    bot_user_id       TEXT    NOT NULL,
    status            TEXT    NOT NULL DEFAULT 'pending',
    last_heartbeat_at TEXT,
    last_error        TEXT,
    created_by        TEXT    NOT NULL,
    created_at        TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_bridges_room_kind ON bridges (room_id, kind);
CREATE INDEX IF NOT EXISTS idx_bridges_bot_user ON bridges (bot_user_id);
