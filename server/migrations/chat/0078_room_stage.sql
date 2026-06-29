-- LC-494: per-room opt-in for "stage" mode (large-audience audio with
-- speakers vs listeners + request-to-speak). Off by default; a room manager
-- turns it on. This migration covers the CONTROL PLANE only - the live
-- speaker/listener roster + request-to-speak is ephemeral (in the WS hub).
-- The actual audio needs an SFU and lands in the LC-512 follow-up. Read via a
-- dedicated getter, like assistant_enabled / retention_days.
ALTER TABLE rooms ADD COLUMN stage_enabled INTEGER NOT NULL DEFAULT 0;
