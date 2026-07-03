-- LC-527: trackable follow-up tasks. A follow-up list is anchored to a
-- `messages` row (mirroring polls, LC-66): the message body is a short header
-- and the interactive checklist renders beneath it. Items are extracted from a
-- call transcript's "## Action items" summary section, but the list itself is a
-- plain per-room card so it works for any room. Each item can be self-claimed
-- (assigned to the claiming user) and checked off; changes fan out over the
-- WebSocket like poll votes.

CREATE TABLE IF NOT EXISTS followups (
    message_id    INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    -- Source call transcript, when the list was created from one (nullable so a
    -- list can be created from any surface later).
    transcript_id INTEGER,
    created_by    TEXT    NOT NULL,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS followup_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL REFERENCES followups(message_id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    text        TEXT    NOT NULL,
    -- Self-claimed assignee (a user id) or NULL when unclaimed. LC-527 is
    -- self-claim only (no assigning work to others), so this is set to the
    -- claiming user's own id.
    assignee_id TEXT,
    done        INTEGER NOT NULL DEFAULT 0,
    done_by     TEXT,
    done_at     TEXT
);

CREATE INDEX IF NOT EXISTS idx_followup_items_message ON followup_items(message_id);
