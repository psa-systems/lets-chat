-- LC-476: per-room politeness gate for @here / @channel broadcast mentions.
-- 'all' (default) preserves today's behavior (anyone who can post may broadcast);
-- 'moderators_only' / 'admins_only' restrict who can fan a mention out to the
-- whole room. Mirrors rooms.posting_allowed_for (LC-85). Enforced at the mention
-- resolver chokepoint; synthetic actors (webhook / email / bridge) bypass it.
ALTER TABLE rooms ADD COLUMN broadcast_allowed_for TEXT NOT NULL DEFAULT 'all'
  CHECK (broadcast_allowed_for IN ('all', 'moderators_only', 'admins_only'));
