-- LC-526 follow-up: per-user opt-out of the public kudos leaderboard. When set,
-- the user is hidden from the /kudos leaderboard (both the "most appreciated"
-- and "most generous" lists). It does NOT stop them from giving or receiving
-- kudos - the recognition message still posts and still notifies; only the
-- public ranking excludes them. OFF by default (everyone is listed).
ALTER TABLE users ADD COLUMN kudos_leaderboard_opt_out INTEGER NOT NULL DEFAULT 0;
