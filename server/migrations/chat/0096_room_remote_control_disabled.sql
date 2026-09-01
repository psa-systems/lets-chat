-- LC-855: per-room off switch for remote control, layered under the
-- workspace-wide `settings.remote_control_enabled` (LC-853). The workspace
-- switch is the master: control is available in a huddle only when the
-- workspace switch is ON *and* the room has not set this flag. A room can only
-- further restrict, never enable something the workspace disabled.
--
-- Default 0 (not disabled), so turning the workspace switch on lights up every
-- room until a room manager opts that room out. Mirrors the stage_enabled /
-- retention_days per-room columns already on this table (0043, 0003).
ALTER TABLE rooms ADD COLUMN remote_control_disabled INTEGER NOT NULL DEFAULT 0
    CHECK (remote_control_disabled IN (0, 1));
