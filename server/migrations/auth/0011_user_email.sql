-- Phase 22 task 4: optional email address per user.
--
-- Needed so the digest tick has a recipient. Nullable because existing
-- users were never asked for one; phase 22 task 5 adds the /settings UI
-- to let users set it. The eligibility query filters out NULL/empty so
-- the digest is silently a no-op for users without an address until they
-- supply one.
--
-- No UNIQUE constraint: this is a notification destination, not an
-- identity field, and lets-chat's identity is the username (with
-- argon2id password / TOTP / session cookies).

ALTER TABLE users ADD COLUMN email TEXT;
