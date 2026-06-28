-- LC-490: message acknowledgement / required-read tracking.
--
-- Two tables mirroring the existing precedents:
--   * message_ack_required is the pin-shaped "this message needs ack" flag
--     (one row per flagged message; presence = required).
--   * message_acks is the reaction-shaped per-user roster of who clicked
--     Acknowledge (composite PK (message_id, user_id)).
-- Clearing the requirement deletes the roster rows (the app does this in one
-- step); ON DELETE CASCADE also drops both when the message is hard-deleted.

CREATE TABLE IF NOT EXISTS message_ack_required (
    message_id  INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    room_id     INTEGER NOT NULL,
    required_by TEXT NOT NULL,
    required_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_ack_required_room ON message_ack_required(room_id);

CREATE TABLE IF NOT EXISTS message_acks (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_message_acks_message ON message_acks(message_id);
