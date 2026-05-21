-- Keep the messages_fts FTS5 index in sync when a `messages` row is
-- hard-deleted.
--
-- The existing FTS triggers in 0008_search.sql fire on INSERT, on
-- UPDATE OF body (edits), and on UPDATE OF deleted_at (soft-delete).
-- None of them fire on a true DELETE. Before per-room retention shipped
-- the codebase only soft-deleted messages, so the gap was invisible;
-- retention introduces the first hard-delete path on `messages`, and
-- without this trigger a swept row leaves an orphan FTS entry that
-- still matches search queries against deleted content.
--
-- The trigger uses the FTS5 'delete' command, which is the documented
-- way to remove a row from a content=messages external-content FTS5
-- table without re-syncing the entire index. Mirrors the
-- soft-delete path in 0008.
CREATE TRIGGER IF NOT EXISTS messages_fts_purge
    AFTER DELETE ON messages
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;
