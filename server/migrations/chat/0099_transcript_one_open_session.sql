-- LC-914: enforce at most one open (status = 'active') transcript session per
-- room. The 0066 index was a plain, non-unique index despite the comment
-- above it claiming "one open session per room", so nothing actually stopped
-- two participants starting within milliseconds of each other from both
-- inserting an active row for the same room.

-- Resolve any pre-existing duplicates first, deterministically: keep the
-- highest-id (most recently opened) active session per room and end the
-- rest, the same transition `end_session` writes on a normal close.
UPDATE call_transcripts
SET status = 'ended', ended_at = datetime('now')
WHERE status = 'active'
  AND id NOT IN (
      SELECT MAX(id) FROM call_transcripts WHERE status = 'active' GROUP BY room_id
  );

DROP INDEX IF EXISTS idx_call_transcripts_open;

-- Now the invariant the comment always claimed is actually enforced: a second
-- INSERT for a room with an active row fails this constraint, so
-- `start_session` can drive its create-vs-join decision off the INSERT result
-- instead of a separate, racy read.
CREATE UNIQUE INDEX idx_call_transcripts_one_open ON call_transcripts (room_id)
    WHERE status = 'active';
