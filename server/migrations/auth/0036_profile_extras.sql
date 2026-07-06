-- LC-533: richer public profile fields shown on the hovercard.
--   pronouns      short free text (e.g. "she/her"), capped on the write path.
--   profile_links newline-separated http(s) URLs (validated + capped on write).
--   timezone      IANA name (e.g. "America/New_York") for the viewer-local
--                 "local time" line; distinct from the DND quiet-hours tz which
--                 stays inside dnd_schedule_json.
-- All three are NULL by default and privacy-gated on read exactly like `bio`.
ALTER TABLE users ADD COLUMN pronouns TEXT;
ALTER TABLE users ADD COLUMN profile_links TEXT;
ALTER TABLE users ADD COLUMN timezone TEXT;
