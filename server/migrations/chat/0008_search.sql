-- FTS5 content table backed by messages
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    body,
    content=messages,
    content_rowid=id
);

-- Populate from existing non-deleted messages
INSERT INTO messages_fts(rowid, body)
    SELECT id, body FROM messages WHERE deleted_at IS NULL;

-- Keep index in sync with the messages table
CREATE TRIGGER IF NOT EXISTS messages_fts_insert
    AFTER INSERT ON messages
BEGIN
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_delete
    AFTER UPDATE OF deleted_at ON messages
    WHEN new.deleted_at IS NOT NULL
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_update
    AFTER UPDATE OF body ON messages
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
