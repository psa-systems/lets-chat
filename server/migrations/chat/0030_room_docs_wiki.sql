-- LC-86: long-form per-room description + a single wiki page per room.
--
-- `description` lives on the room info page next to the (already-existing)
-- `topic` column; topic stays short for the header, description holds the
-- "what is this channel for" paragraph.
--
-- `wiki_body` is the single wiki page (multi-page wikis are deferred per
-- the open questions; one page covers the announcement / docs use case).
-- `wiki_updated_at` and `wiki_updated_by` carry the last-edit metadata so
-- the info page can show "Last updated by X on YYYY-MM-DD HH:MM:SS"
-- without an audit-log lookup. The audit row still lands in `mod_actions`
-- for cross-correlation.
--
-- All three columns are nullable. Existing rooms come out unchanged
-- because the ALTER does not back-fill, and the absence of a wiki body
-- is the "no wiki yet" signal the template branches on.

ALTER TABLE rooms ADD COLUMN description TEXT;
ALTER TABLE rooms ADD COLUMN wiki_body TEXT;
ALTER TABLE rooms ADD COLUMN wiki_updated_at TEXT;
ALTER TABLE rooms ADD COLUMN wiki_updated_by TEXT;
