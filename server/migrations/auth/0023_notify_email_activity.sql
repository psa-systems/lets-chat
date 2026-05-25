-- LC-77-REPLY (#201) stage 1: per-user opt-in for per-message mention + DM
-- email notifications. OFF by default - mention emails are noisy and an
-- operator opts users in deliberately, same conservative default
-- `notify_email_digest_enabled` (chat/0013_digest_columns) uses.
--
-- The per-room mute + DND state still gate at dispatch time via the same
-- helpers `push::dispatch` consults; this column only toggles whether the
-- email surface is eligible at all.
ALTER TABLE users ADD COLUMN notify_email_activity_enabled INTEGER NOT NULL DEFAULT 0;
