-- LC-1135: proposed YouTrack issues drawn from a call transcript.
--
-- After a call, the LLM proposes issues (summary + a plain-text assignee name +
-- the transcript source lines each came from) and they are stored here as
-- `proposed`. A reviewer accepts the ones they want and files them as real
-- YouTrack issues in one step; filing flips the row to `filed` and records the
-- created issue's readable key, who filed it, and when. Regeneration replaces
-- only the still-`proposed` rows, so already-filed proposals are never lost or
-- re-filed. Nothing is filed without the human accept.
CREATE TABLE IF NOT EXISTS transcript_issue_proposals (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    transcript_id     INTEGER NOT NULL,
    summary           TEXT    NOT NULL,
    assignee_name     TEXT,
    source_lines      TEXT    NOT NULL DEFAULT '',
    -- 'proposed' | 'filed' | 'dismissed'
    status            TEXT    NOT NULL DEFAULT 'proposed',
    -- The created issue's readable key (e.g. 'LC-1234'); set only when filed.
    youtrack_issue_id TEXT,
    filed_by          TEXT,
    filed_at          TEXT,
    created_at        TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- The review panel reads every proposal for one transcript, newest first.
CREATE INDEX IF NOT EXISTS idx_transcript_issue_proposals_tid
    ON transcript_issue_proposals (transcript_id, id);
