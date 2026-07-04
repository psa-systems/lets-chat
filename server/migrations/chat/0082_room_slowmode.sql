-- LC-534: per-channel slowmode. A room manager sets a minimum interval
-- (seconds) between a member's posts; 0 (default) = off. Enforced in
-- post_message via the in-memory cooldown limiter. Moderators are exempt.
ALTER TABLE rooms ADD COLUMN slowmode_seconds INTEGER NOT NULL DEFAULT 0;
