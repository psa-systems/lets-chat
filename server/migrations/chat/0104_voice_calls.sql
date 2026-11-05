-- LC-1113 Phase 1: group the voice-call lifecycle log (voice_events) into calls.
--
-- A "call" is one span during which a voice room is non-empty: opened when the
-- room goes 0 -> 1 participant and closed when it goes 1 -> 0 (the same
-- empty-room point that finalizes a transcript). voice_events.call_id links each
-- event (connect / drop / mute / error / ...) to its call, so the admin
-- observability view can show one call's whole timeline instead of a flat
-- firehose, which is what an admin needs to trace why a single call misbehaved.
CREATE TABLE IF NOT EXISTS voice_calls (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id    INTEGER NOT NULL,
    started_at TEXT    NOT NULL DEFAULT (datetime('now')),
    ended_at   TEXT
);

-- At most one OPEN (ended_at IS NULL) call per room, so a join reliably finds
-- the single call to attach its events to.
CREATE UNIQUE INDEX IF NOT EXISTS idx_voice_calls_one_open
    ON voice_calls (room_id) WHERE ended_at IS NULL;

-- The admin "recent calls" listing reads newest-first.
CREATE INDEX IF NOT EXISTS idx_voice_calls_started ON voice_calls (started_at DESC);

-- Nullable: rows written before this migration, or any event logged while no
-- call is open, keep call_id NULL and still show in the flat log.
ALTER TABLE voice_events ADD COLUMN call_id INTEGER;

-- Serves the per-call drill-down (all events for one call, chronological).
CREATE INDEX IF NOT EXISTS idx_voice_events_call ON voice_events (call_id, created_at, id);
