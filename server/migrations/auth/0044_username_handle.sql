-- LC-766: users choose their chat handle deliberately and can change it later.
--
-- username_confirmed_at is NULL when the handle was derived by pick_username
-- (the email local part or the preferred_username claim) and the user has never
-- confirmed it. That NULL is what fires the first-entry prompt; a timestamp
-- means they picked or accepted a handle and the prompt stays quiet.
ALTER TABLE users ADD COLUMN username_confirmed_at TEXT;

-- username_changed_at records the last deliberate handle change, feeding the
-- per-account change cooldown. NULL means the handle has never been changed
-- (the first-entry pick does not count as a change and leaves this NULL).
ALTER TABLE users ADD COLUMN username_changed_at TEXT;

-- Grandfather every existing account as confirmed: they already have a handle
-- and must not be forced through the first-entry prompt. Only accounts
-- provisioned AFTER this migration (which insert NULL) get prompted.
UPDATE users SET username_confirmed_at = datetime('now') WHERE username_confirmed_at IS NULL;

-- A released handle is reserved for its previous owner for a fixed window so a
-- mention or profile link typed against the old handle cannot resolve to a
-- different person who claimed it in the meantime. The previous owner may
-- reclaim their own reserved handle; anyone else is refused until
-- reserved_until has passed. COLLATE NOCASE mirrors users.username so
-- reservations are case-insensitive too.
CREATE TABLE reserved_usernames (
    username       TEXT PRIMARY KEY COLLATE NOCASE,
    user_id        TEXT NOT NULL,
    reserved_until TEXT NOT NULL
);

-- Supports pruning expired reservations by timestamp.
CREATE INDEX idx_reserved_usernames_until ON reserved_usernames(reserved_until);
