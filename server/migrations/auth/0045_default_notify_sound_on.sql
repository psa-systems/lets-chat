-- LC-1120: notify sound now defaults on (call cues: the incoming-call ring and
-- the join/leave cue for a voice room). New accounts already insert
-- notify_sound_enabled = 1; flip every existing human account on too so the
-- change reaches the current users, not only future signups.
--
-- This deliberately overrides an earlier off value: the old default was 0, so a
-- 0 cannot be distinguished from a conscious opt-out, and the intent is a
-- blanket on. Bots carry no UI or session and are left as-is.
UPDATE users SET notify_sound_enabled = 1 WHERE is_bot = 0;
