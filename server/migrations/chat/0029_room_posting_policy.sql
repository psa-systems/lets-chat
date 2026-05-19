-- LC-85: per-room "who can post" policy. Default 'all' preserves the
-- pre-feature behavior (every member of the room can post); admins flip
-- a room to 'moderators_only' or 'admins_only' for announcement-style
-- channels. The CHECK constraint blocks typos; backfill is implicit via
-- the DEFAULT so existing rooms come out unchanged.
--
-- Effective-role gate (LC-84) decides whether a caller meets the bar:
-- `effective_role(room, user)` of `moderator` or `admin` clears
-- `moderators_only`; only `admin` clears `admins_only`.

ALTER TABLE rooms ADD COLUMN posting_allowed_for TEXT NOT NULL
    DEFAULT 'all'
    CHECK (posting_allowed_for IN ('all', 'moderators_only', 'admins_only'));
