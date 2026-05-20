-- LC-62: scheduled message delivery.
--
-- A row sits here from compose time until the dispatcher delivers it. State
-- derives from delivered_at + failed_reason at every query site (no enum
-- column, so the predicates stay in one layer):
--   pending   = delivered_at IS NULL
--   delivered = delivered_at IS NOT NULL AND failed_reason IS NULL
--   dropped   = delivered_at IS NOT NULL AND failed_reason IS NOT NULL
--
-- room_id ON DELETE CASCADE: room gone -> no destination -> drop the row.
-- parent_id / quote_id / file_id ON DELETE SET NULL: referenced object gone
-- -> deliver text-only / top-level / unquoted, do not fail.
-- No FK on user_id: auth lives in a separate pool. routes/account.rs
-- ::purge_user_chat deletes by user_id in the user-purge transaction.
CREATE TABLE IF NOT EXISTS scheduled_messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id         INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id         TEXT    NOT NULL,
    body            TEXT    NOT NULL,
    scheduled_for   TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    parent_id       INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    quote_id        INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    file_id         INTEGER REFERENCES file_uploads(id) ON DELETE SET NULL,
    delivered_at    TEXT,
    failed_reason   TEXT,
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT
);

-- Dispatcher tick: due rows not in backoff cooldown, oldest first. Partial
-- index keeps the scan cheap once the table fills with delivered rows.
CREATE INDEX IF NOT EXISTS idx_sched_due
    ON scheduled_messages (scheduled_for)
    WHERE delivered_at IS NULL;

-- `/scheduled` management page: per-user pending list, soonest first.
CREATE INDEX IF NOT EXISTS idx_sched_user
    ON scheduled_messages (user_id, scheduled_for)
    WHERE delivered_at IS NULL;

-- Orphan-sweep protection: db::uploads::select_orphans_older_than uses
-- NOT EXISTS against this table to skip uploads still referenced by a
-- pending scheduled row.
CREATE INDEX IF NOT EXISTS idx_sched_file
    ON scheduled_messages (file_id)
    WHERE file_id IS NOT NULL AND delivered_at IS NULL;
