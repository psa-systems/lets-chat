-- Guard the FTS purge trigger added in 0045 against the soft-delete +
-- hard-delete interaction.
--
-- 0045 fires `INSERT INTO messages_fts(messages_fts) VALUES ('delete',
-- old.id, old.body)` on every DELETE on `messages`. If the row was
-- previously soft-deleted, the 0008 `messages_fts_delete` trigger
-- already issued the same FTS5 'delete' command on UPDATE OF
-- deleted_at, removing the FTS row at that point. When the retention
-- sweep later hard-deletes the same row, 0045's trigger fires the
-- 'delete' command a second time against an FTS rowid that no longer
-- exists, and FTS5 reports it as `SQLITE_CORRUPT` ("database disk
-- image is malformed"). The disk is fine; FTS5 uses that error code
-- for "you told me to delete a rowid that is not in the index."
--
-- Surfaced by `tests/retention_sweep.rs::soft_deleted_message_past_
-- cutoff_is_hard_deleted`, which is the integration test for the
-- settled decision that soft-deleted messages are not exempt from
-- retention. The cascade tests in 0045's batch missed the interaction
-- because they only hard-deleted messages that were never soft-deleted.
--
-- Fix: guard the trigger with `WHEN old.deleted_at IS NULL` so it only
-- fires when the row was still indexed in messages_fts at the moment
-- of DELETE. This matches the data-model invariant established by
-- 0008: FTS contains exactly the non-soft-deleted messages.
--
-- DROP + CREATE because SQLite cannot ALTER a trigger; idempotent via
-- IF EXISTS / IF NOT EXISTS so a re-apply is a no-op.

DROP TRIGGER IF EXISTS messages_fts_purge;

CREATE TRIGGER IF NOT EXISTS messages_fts_purge
    AFTER DELETE ON messages
    WHEN old.deleted_at IS NULL
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;
