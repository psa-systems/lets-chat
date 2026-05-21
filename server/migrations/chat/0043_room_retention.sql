-- Per-room message retention policy. NULL means no retention (default);
-- a positive integer N means "hard-delete messages in this room older
-- than N days" on the next retention sweep tick.
--
-- Floor at 1 day via CHECK prevents an admin from accidentally setting
-- retention_days=0 ("delete everything immediately") through a
-- fat-fingered form post. A genuine "empty this room" need has a
-- different feature (admin delete room).
--
-- The partial index keeps the global retention sweep query cheap: the
-- vast majority of rooms have no retention configured, so we want the
-- sweep to walk only the configured ones rather than scan all rooms.
--
-- DMs (room_type='dm') technically inherit this column because DMs are
-- rows in this same `rooms` table (see 0003_dms.sql). The retention
-- sweep predicate explicitly excludes them (`room_type != 'dm'`) so
-- this column is unreachable by the sweep for DM rows even if a value
-- ever lands there. DM retention is a separate, deferred feature.
ALTER TABLE rooms ADD COLUMN retention_days INTEGER
    CHECK (retention_days IS NULL OR retention_days >= 1);

CREATE INDEX IF NOT EXISTS idx_rooms_retention_days
    ON rooms(retention_days)
    WHERE retention_days IS NOT NULL;
