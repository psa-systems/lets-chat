-- LC-66: polls / voting. A poll is anchored to a `messages` row: the
-- message body holds the question (so search / quote-reply / pinning work),
-- and the poll block renders the options + tallies beneath it.
--
-- All three tables cascade from the message: deleting the message removes
-- the poll, its options, and every vote (FK enforcement is on via the
-- sqlx connect default). closed_at is NULL until the poll closes (either a
-- background tick once closes_at passes, or never for an open-ended poll).
CREATE TABLE IF NOT EXISTS polls (
    message_id   INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    question     TEXT    NOT NULL,
    allows_multi INTEGER NOT NULL DEFAULT 0,
    anonymous    INTEGER NOT NULL DEFAULT 0,
    closes_at    TEXT,
    closed_at    TEXT
);

CREATE TABLE IF NOT EXISTS poll_options (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES polls(message_id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    text       TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_poll_options_msg
    ON poll_options (message_id, position);

CREATE TABLE IF NOT EXISTS poll_votes (
    option_id INTEGER NOT NULL REFERENCES poll_options(id) ON DELETE CASCADE,
    user_id   TEXT    NOT NULL,
    voted_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (option_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_poll_votes_user ON poll_votes (user_id);

-- Background closer scans for open polls whose closes_at has passed.
CREATE INDEX IF NOT EXISTS idx_polls_closing
    ON polls (closes_at) WHERE closed_at IS NULL AND closes_at IS NOT NULL;
