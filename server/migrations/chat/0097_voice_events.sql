-- LC-859: server-side observability log for the voice/huddle call lifecycle.
--
-- The mute desync fixed in LC-764 was diagnosed only from participants
-- describing what they could hear, because the server kept no record of who
-- connected, dropped, rejoined, or toggled mute during a live call. This table
-- is that record: one append-only row per server-observable lifecycle event
-- (connect / reconnect / left / dropped / mute / unmute), each carrying the
-- room id, the participant id (and a denormalized label so the log stays
-- readable after a rename/delete), an optional detail, and a timestamp. An
-- admin reads it live from /admin/voice-log while a meeting is still running.
--
-- No FK on user_id: it is an auth.db id and this is chat.db (separate pool),
-- matching remote_control_events and the other cross-domain ids stored here.
-- Not a content log: it never stores SDP/ICE payloads or message text.
CREATE TABLE IF NOT EXISTS voice_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id    INTEGER NOT NULL,
    user_id    TEXT    NOT NULL,
    user_label TEXT    NOT NULL DEFAULT '',
    kind       TEXT    NOT NULL,
    detail     TEXT,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- The admin listing reads newest-first; this index serves that ORDER BY.
CREATE INDEX IF NOT EXISTS idx_voice_events_created ON voice_events (created_at DESC);
-- Reconnect detection asks "did this user just leave this room?"; this index
-- serves that per-(room,user) recency lookup.
CREATE INDEX IF NOT EXISTS idx_voice_events_room_user
    ON voice_events (room_id, user_id, created_at DESC);
