-- Phase 22 task 2: typed SMTP settings with at-rest-encrypted password.
--
-- Replaces the per-key rows in `settings` (smtp_host, smtp_port, smtp_user,
-- smtp_pass, smtp_from). Host/port/user/from are preserved across the upgrade
-- so the operator does not have to re-enter the network config. The stored
-- plaintext password is dropped: the new column requires an AES-256-GCM
-- ciphertext under LETS_CHAT_SECRET_KEY, so the operator re-enters it in the
-- admin form after restart. Documented as a one-shot upgrade step in README.

CREATE TABLE IF NOT EXISTS smtp_settings (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    host                TEXT    NOT NULL DEFAULT '',
    port                INTEGER NOT NULL DEFAULT 587,
    username            TEXT,
    password_encrypted  BLOB,
    password_nonce      BLOB,
    from_address        TEXT    NOT NULL DEFAULT '',
    tls_mode            TEXT    NOT NULL DEFAULT 'starttls',
    updated_at          TEXT    NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO smtp_settings (id, host, port, username, from_address, tls_mode)
SELECT 1,
       COALESCE((SELECT value FROM settings WHERE key = 'smtp_host'), ''),
       COALESCE(
           CAST(NULLIF((SELECT value FROM settings WHERE key = 'smtp_port'), '') AS INTEGER),
           587
       ),
       NULLIF((SELECT value FROM settings WHERE key = 'smtp_user'), ''),
       COALESCE((SELECT value FROM settings WHERE key = 'smtp_from'), ''),
       'starttls'
WHERE NOT EXISTS (SELECT 1 FROM smtp_settings WHERE id = 1);

DELETE FROM settings
 WHERE key IN ('smtp_host', 'smtp_port', 'smtp_user', 'smtp_pass', 'smtp_from');
