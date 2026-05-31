-- LC-207-OBSERVABILITY (#278): operator-visible IMAP poll-loop health.
-- Singleton row (id = 1) updated by spawn_email_poll on every tick so the
-- admin settings page can answer "is the poll loop alive, when did it last
-- run, and is it erroring?" without container-log access. Operator-entered
-- config stays in imap_inbox_config (0005/0007); this table is runtime status
-- only (no credentials, no message content).
CREATE TABLE IF NOT EXISTS imap_poll_status (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_poll_at TEXT,
    last_ok_at TEXT,
    last_error TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_fetched INTEGER NOT NULL DEFAULT 0,
    last_posted INTEGER NOT NULL DEFAULT 0,
    last_dropped INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
