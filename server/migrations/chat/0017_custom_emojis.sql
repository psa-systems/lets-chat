CREATE TABLE IF NOT EXISTS custom_emojis (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id   INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    shortcode    TEXT    NOT NULL,
    storage_path TEXT    NOT NULL,
    mime_type    TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    uploaded_by  TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (enclave_id, shortcode)
);

CREATE INDEX IF NOT EXISTS idx_custom_emojis_enclave ON custom_emojis(enclave_id);
