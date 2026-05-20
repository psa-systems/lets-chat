-- LC-63: message reminders. A user picks a time to be pinged about a
-- specific message; a background poller fires due reminders through the
-- existing notification pipeline (Web Push + on-screen banner) with a
-- deep link back to the message.
--
-- message_id REFERENCES messages ON DELETE CASCADE: if the message is
-- deleted the reminder is removed and never fires (acceptance criterion).
-- fired_at is NULL until the reminder fires; the poller claims a row by
-- flipping it to datetime('now'), and a dispatch failure leaves it NULL
-- so the next tick retries.
--
-- remind_at is stored in SQLite's "YYYY-MM-DD HH:MM:SS" UTC shape, matching
-- the other datetime columns, so `remind_at <= datetime('now')` is a plain
-- string comparison.
CREATE TABLE IF NOT EXISTS reminders (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    TEXT    NOT NULL,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    remind_at  TEXT    NOT NULL,
    fired_at   TEXT,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Dispatcher poll: due, not-yet-fired rows.
CREATE INDEX IF NOT EXISTS idx_reminders_due
    ON reminders (remind_at) WHERE fired_at IS NULL;

-- Per-user listing (pending soonest-first, fired most-recent-first).
CREATE INDEX IF NOT EXISTS idx_reminders_user
    ON reminders (user_id, remind_at);
