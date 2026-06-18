-- LC-342: "shame tag" community-moderation prototype.
--
-- Members of a shame-tags-enabled enclave vote moderation tags onto a message
-- (spam / abusive / off-topic / misinformation). Once a hide-worthy tag passes
-- a vote threshold within the aging window, the message renders default-hidden
-- behind a click-through. A moderator can override (force show / force hide).
-- Prototype, opt-in per enclave (dark-ship); thresholds/taxonomy are code-side.

-- Per-enclave opt-in (mirrors coyote_mode / share_emojis_globally). Off = the
-- whole shame-tag UI + hiding is inert for the enclave's rooms.
ALTER TABLE enclaves ADD COLUMN shame_tags_enabled INTEGER NOT NULL DEFAULT 0;

-- One vote per (message, tag, voter). Count per (message, tag) is the signal;
-- created_at lets old votes age out of the aggregate.
CREATE TABLE message_tags (
    message_id    INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    tag           TEXT    NOT NULL,
    voter_user_id TEXT    NOT NULL,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, tag, voter_user_id)
);

-- Moderator override of the community decision. hidden=1 force-hide, hidden=0
-- force-show; absence = let the vote threshold decide. One row per message.
CREATE TABLE message_tag_overrides (
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    hidden     INTEGER NOT NULL,
    actor_user TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);
