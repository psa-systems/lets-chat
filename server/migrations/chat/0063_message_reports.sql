-- LC-334: member-filed reports on messages, triaged in the site-admin queue
-- (/admin/reports). reporter_id / handled_by are auth-db user ids (TEXT);
-- room_id is denormalized from the message so the queue can show the room and
-- build a jump link without a cross-table join, and survives a soft-deleted
-- message. UNIQUE(message_id, reporter_id) makes a re-report by the same user a
-- no-op rather than a duplicate row.
CREATE TABLE message_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL,
    room_id INTEGER NOT NULL,
    reporter_id TEXT NOT NULL,
    category TEXT NOT NULL,
    note TEXT,
    status TEXT NOT NULL DEFAULT 'open',
    handled_by TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    handled_at TEXT,
    UNIQUE (message_id, reporter_id),
    FOREIGN KEY (message_id) REFERENCES messages (id) ON DELETE CASCADE
);

-- The queue reads open reports newest-first.
CREATE INDEX idx_message_reports_status ON message_reports (status, created_at DESC);
