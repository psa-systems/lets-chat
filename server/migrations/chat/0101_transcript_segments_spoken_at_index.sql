-- LC-928: `recent_texts_by_others` filters transcript_segments by
-- (transcript_id, spoken_at >= cutoff) but the only existing index is
-- (transcript_id, id), which is useless for the spoken_at predicate. SQLite
-- was walking the id-ordered index backward evaluating spoken_at row by row;
-- because the LIMIT 20 rarely satisfies early (matches cluster at the very
-- end), the walk covered the entire remaining call history on every caption.
-- This index lets SQLite seek straight to the spoken_at cutoff instead of
-- scanning from the newest row down to it.
CREATE INDEX IF NOT EXISTS idx_transcript_segments_tid_spoken_at
    ON transcript_segments (transcript_id, spoken_at);
