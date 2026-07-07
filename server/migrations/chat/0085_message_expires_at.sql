-- LC-547: ephemeral / self-destruct messages. A message with a non-NULL
-- expires_at is hard-deleted by the unconditional ephemeral sweep once
-- expires_at <= now (see retention::sweep::sweep_expired_ephemeral). NULL means
-- permanent, which is the default for every existing row and every message the
-- sender did not attach a timer to. Stored as a "%Y-%m-%d %H:%M:%S" UTC string,
-- the same shape as datetime('now'), so `expires_at <= datetime('now')` is a
-- correct lexicographic comparison.
--
-- The partial index keeps the sweep's scan proportional to the (small) set of
-- live ephemeral messages instead of the whole messages table.
ALTER TABLE messages ADD COLUMN expires_at TEXT;

CREATE INDEX idx_messages_expires_at ON messages (expires_at) WHERE expires_at IS NOT NULL;
