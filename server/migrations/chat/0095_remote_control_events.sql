-- LC-855: append-only audit of every remote-control consent event, not only
-- the granted sessions the LC-186 `remote_control_sessions` table records.
--
-- remote_control_sessions captures the *session* lifecycle (open on grant,
-- close on revoke) so a live/handed-over control session is traceable. It does
-- NOT capture the requests that were never granted, or the explicit denials -
-- the parts an audit reviewer most wants when asking "who tried to control
-- whom". This table logs one row per consent event (request / grant / deny /
-- revoke) from both the 1:1 DM relay and the huddle relay, with actor, target,
-- room, and timestamp. Append-only: rows are never updated or deleted here (the
-- session table owns the ended_at/end_reason mutation).
--
-- No FK on the ids: actor_id / target_id are auth.db user ids and this is
-- chat.db (separate pool), matching remote_control_sessions and the other
-- cross-domain ids stored in this database.
CREATE TABLE IF NOT EXISTS remote_control_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id    INTEGER NOT NULL,
    actor_id   TEXT    NOT NULL,
    target_id  TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- The admin listing reads newest-first; the index serves that ORDER BY.
CREATE INDEX IF NOT EXISTS idx_rce_created ON remote_control_events (created_at DESC);
