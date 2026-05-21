-- Rebuild link_filter_quarantine so its FK to messages(id) declares
-- ON DELETE CASCADE.
--
-- The original schema in 0032_anti_spam.sql declared the FK without an
-- ON DELETE action (defaulting to NO ACTION), with a comment baking in
-- the assumption that "messages are soft-deleted in this codebase, not
-- hard-deleted, so a cascade would never fire in practice." Per-room
-- message retention is the first hard-delete path on messages; without
-- this rebuild, a retention sweep that touched a quarantined message
-- would fail with SQLITE_CONSTRAINT_FOREIGNKEY and roll back the whole
-- batch.
--
-- SQLite does not support `ALTER TABLE ... ADD CONSTRAINT` or otherwise
-- modifying an existing FK action, so the rebuild is: create a new
-- table with the correct FK, copy rows over, drop the old, rename.
-- No other tables reference link_filter_quarantine, so the rename does
-- not require additional FK fix-up.

CREATE TABLE link_filter_quarantine_new (
    message_id      INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    matched_pattern TEXT NOT NULL,
    matched_url     TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    reviewed_by     TEXT,
    reviewed_at     TEXT,
    decision        TEXT CHECK (decision IN ('approve', 'reject'))
);

INSERT INTO link_filter_quarantine_new
    (message_id, matched_pattern, matched_url, created_at, reviewed_by, reviewed_at, decision)
SELECT message_id, matched_pattern, matched_url, created_at, reviewed_by, reviewed_at, decision
FROM link_filter_quarantine;

DROP TABLE link_filter_quarantine;

ALTER TABLE link_filter_quarantine_new RENAME TO link_filter_quarantine;

CREATE INDEX IF NOT EXISTS idx_quarantine_unreviewed
    ON link_filter_quarantine(created_at)
    WHERE reviewed_at IS NULL;
