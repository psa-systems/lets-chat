-- LC-77: encrypted-at-rest IMAP-poll configuration for the email-ingress
-- feature. Mirrors the VAPID singleton pattern (vapid_keypair, settings/0003):
-- one row, id = 1, with sensitive credential bytes sealed via crypto::seal
-- (AES-256-GCM keyed by LETS_CHAT_SECRET_KEY) and the public-shape fields
-- in plaintext.
--
-- The IMAP poll loop reads this row at startup and refuses to spawn if any
-- of {host, username, password_encrypted, ingress_domain} is unset. Flipping
-- `enabled` to 1 (or setting fields on a fresh row) requires a server
-- restart to take effect; the spawn function checks at startup, not per tick,
-- matching the LETS_CHAT_RETENTION_SWEEP_ENABLED + retention sweeper shape.
--
-- The SMTP password counterpart in `settings.smtp_pass` is currently stored
-- plaintext. Migrating SMTP to this same sealed pattern is tracked as a
-- separate followup (LC-77-SMTP-SEAL); not in scope for LC-77 v1.
CREATE TABLE IF NOT EXISTS imap_inbox_config (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    host                TEXT    NOT NULL,
    port                INTEGER NOT NULL,
    tls                 INTEGER NOT NULL DEFAULT 1,
    username            TEXT    NOT NULL,
    password_encrypted  BLOB    NOT NULL,
    password_nonce      BLOB    NOT NULL,
    folder              TEXT    NOT NULL DEFAULT 'INBOX',
    ingress_domain      TEXT,
    enabled             INTEGER NOT NULL DEFAULT 0,
    updated_at          TEXT    NOT NULL DEFAULT (datetime('now'))
);
