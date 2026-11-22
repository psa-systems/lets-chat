-- Track when a proposal was claimed for filing, so a stale
-- `filing` row (one whose claimer was interrupted before resolving it to
-- `filed` or back to `proposed`) can be swept back to `proposed` after a
-- fixed timeout instead of staying stuck forever.
ALTER TABLE transcript_issue_proposals ADD COLUMN claimed_at TEXT;
