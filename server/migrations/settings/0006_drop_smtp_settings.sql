-- LC-77-SMTP-SEAL: drop the five stale SMTP rows that the admin form used
-- to write to `settings`. They were write-only: `mail::Mailer::from_env`
-- reads SMTP config exclusively from the `SMTP_*` env vars, so these rows
-- never affected outbound mail. The admin form has been removed; this
-- migration clears the data so a `settings.db` leak no longer contains
-- the plaintext SMTP password an operator may have entered into the
-- earlier UI.
--
-- Idempotent: keys that aren't present produce zero deletes. Operators
-- who never used the form aren't affected.
DELETE FROM settings
WHERE key IN ('smtp_host', 'smtp_port', 'smtp_user', 'smtp_pass', 'smtp_from');
