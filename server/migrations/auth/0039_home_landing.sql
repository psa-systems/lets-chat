-- LC-575: "Open on" landing preference. Chooses what an authenticated user
-- sees at `/`: their last-visited room (today's behavior) or the Home
-- dashboard. NULL = "last-room", so every existing user keeps the current
-- redirect until they opt in. Values: "last-room" | "home".
ALTER TABLE users ADD COLUMN home_landing TEXT;
