-- LC-186: audit log for remote-control sessions (the TeamViewer-style feature,
-- LC-181). One row per granted session so a grant is traceable: who controlled
-- whom, in which DM room, and for how long.
--
-- A row is opened when the sharer grants control (the `grant` signal relayed in
-- routes::ws::relay_control_signal) and closed when control ends. Ends arrive
-- as a `revoke` signal (kill-switch, auto-revoke, or the controlled side's
-- teardown all emit one) and, as a backstop for a hard WS disconnect that never
-- sent a revoke, on socket close. `ended_at IS NULL` therefore means either a
-- still-live session or one whose server crashed mid-session; it is not a leak
-- (no message content is stored, only the participant ids + timestamps).
--
-- No FK on the ids: controller_id / sharer_id are auth.db user ids and this is
-- chat.db (separate pool), matching how other cross-domain ids are stored here.
CREATE TABLE IF NOT EXISTS remote_control_sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id       INTEGER NOT NULL,
    controller_id TEXT    NOT NULL,
    sharer_id     TEXT    NOT NULL,
    started_at    TEXT    NOT NULL DEFAULT (datetime('now')),
    ended_at      TEXT,
    end_reason    TEXT
);

-- Closing a session looks up the open row for a room; partial index keeps that
-- lookup cheap and there is at most one open row per DM room at a time.
CREATE INDEX IF NOT EXISTS idx_rcs_open ON remote_control_sessions (room_id)
    WHERE ended_at IS NULL;
