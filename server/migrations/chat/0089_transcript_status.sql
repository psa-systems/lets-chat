-- LC-590: outcome of the last server-side transcription attempt for this
-- upload. NULL means "never attempted, or not transcribable" (every pre-LC-590
-- row, plus every image / plain file). 'pending' is set when a retry is queued,
-- 'done' alongside a stored transcript, and 'failed' once the retry policy is
-- exhausted - which is what makes the attachment render its Retry control
-- instead of silently showing nothing.
ALTER TABLE file_uploads ADD COLUMN transcript_status TEXT;
