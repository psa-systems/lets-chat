-- LC-207-OBSERVABILITY (#278): operator-visible retention-sweep status.
-- Singleton row (id = 1) updated by spawn_message_retention_sweeper on every
-- tick (including zero-delete ticks) so an operator who enabled the
-- destructive LETS_CHAT_RETENTION_SWEEP_ENABLED can confirm from the admin
-- settings page that the sweep ran and how much it deleted, without
-- container-log access. last_run_at tracks the last completed (Ok) tick;
-- last_error tracks the most recent failure.
CREATE TABLE IF NOT EXISTS retention_sweep_status (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at TEXT,
    last_rooms_touched INTEGER NOT NULL DEFAULT 0,
    last_messages_deleted INTEGER NOT NULL DEFAULT 0,
    total_messages_deleted INTEGER NOT NULL DEFAULT 0,
    runs INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
