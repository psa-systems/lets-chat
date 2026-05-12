ALTER TABLE sessions ADD COLUMN user_agent TEXT;
ALTER TABLE sessions ADD COLUMN ip TEXT;
ALTER TABLE sessions ADD COLUMN last_seen_at TEXT;

CREATE INDEX IF NOT EXISTS idx_sessions_last_seen ON sessions(last_seen_at);
