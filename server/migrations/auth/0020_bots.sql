-- LC-73: first-class bot identity. A bot is a `users` row with is_bot = 1.
-- Bots authenticate only via API tokens (LC-72); the cookie login path
-- rejects them and they have an empty (non-verifiable) password hash. The
-- first-user-is-admin rule excludes bots so a bot cannot become admin by
-- being the first registration.
ALTER TABLE users ADD COLUMN is_bot INTEGER NOT NULL DEFAULT 0;
