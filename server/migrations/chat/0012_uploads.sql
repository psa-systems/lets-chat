CREATE TABLE IF NOT EXISTS file_uploads (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    uploader_id  TEXT NOT NULL,
    message_id   INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    filename     TEXT NOT NULL,
    mime_type    TEXT NOT NULL,
    size_bytes   INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_file_uploads_message ON file_uploads(message_id);
-- Orphan GC: a future cron sweeps rows with message_id IS NULL older than N minutes.
CREATE INDEX IF NOT EXISTS idx_file_uploads_orphan
    ON file_uploads(created_at)
    WHERE message_id IS NULL;
