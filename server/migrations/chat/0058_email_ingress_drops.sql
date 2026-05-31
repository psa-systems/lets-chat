-- LC-207-OBSERVABILITY (#278): rolling log of email-ingress message drops so
-- an operator can answer "why didn't my email post?" from the admin settings
-- page instead of grepping container logs. Stores the structured drop reason,
-- the IMAP UID, and a bounded non-body diagnostic detail only - never the
-- message body, subject, or any correspondent address. Swept at 30 days by the
-- hourly orphan sweeper, mirroring the dedup table (0051).
CREATE TABLE IF NOT EXISTS email_ingress_drops (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    dropped_at TEXT NOT NULL DEFAULT (datetime('now')),
    reason TEXT NOT NULL,
    uid INTEGER,
    detail TEXT
);

CREATE INDEX IF NOT EXISTS idx_email_ingress_drops_dropped_at
    ON email_ingress_drops (dropped_at DESC);
