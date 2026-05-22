-- LC-88: per-user Do Not Disturb (quiet hours + manual pause).
--
-- dnd_schedule_json: JSON describing recurring quiet hours, or NULL for no
--   schedule. Shape:
--     {"timezone":"America/New_York",
--      "weekday":{"start":"22:00","end":"07:00"},
--      "weekend":{"start":"00:00","end":"09:00"}}
--   Either group may be null/absent to leave that day-type unsuppressed.
--   Windows where start > end span midnight (e.g. 22:00->07:00).
--
-- dnd_paused_until: ISO-8601 UTC instant of an explicit manual pause, or NULL.
--   When set and in the future it supersedes the schedule. Auto-expires by
--   simply being in the past; no sweeper needed.
ALTER TABLE users ADD COLUMN dnd_schedule_json TEXT;
ALTER TABLE users ADD COLUMN dnd_paused_until TEXT;
