-- LC-393: meeting transcription for 1:1 DM calls (Phase 1).
--
-- Call media is peer-to-peer WebRTC; the server never sees the audio, so
-- transcription is captured CLIENT-SIDE (each participant transcribes their own
-- microphone) and posted here. A `call_transcripts` row is one transcription
-- session, opened when a participant turns transcription on for a call and
-- closed on hangup (or by the WS-disconnect backstop, mirroring LC-186's
-- remote_control_sessions). Each `transcript_segments` row is one final speech
-- result; the speaker is simply the poster, because each browser only ever
-- transcribes its own mic - no diarization needed.
--
-- No FK on the user ids: started_by / user_id are auth.db ids and this is
-- chat.db (separate pool), matching the cross-domain-id convention used by
-- remote_control_sessions (0052). The transcript_id FK is intra-db and safe.
CREATE TABLE IF NOT EXISTS call_transcripts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id    INTEGER NOT NULL,
    started_by TEXT    NOT NULL,
    started_at TEXT    NOT NULL DEFAULT (datetime('now')),
    ended_at   TEXT,
    status     TEXT    NOT NULL DEFAULT 'active'
);

-- One open session per room at a time; partial index keeps the "is there an
-- open session for this call?" lookup cheap (same shape as idx_rcs_open).
CREATE INDEX IF NOT EXISTS idx_call_transcripts_open ON call_transcripts (room_id)
    WHERE status = 'active';

CREATE TABLE IF NOT EXISTS transcript_segments (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    transcript_id INTEGER NOT NULL,
    user_id       TEXT    NOT NULL,
    text          TEXT    NOT NULL,
    spoken_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (transcript_id) REFERENCES call_transcripts (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_transcript_segments_tid
    ON transcript_segments (transcript_id, id);
