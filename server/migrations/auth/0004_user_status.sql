ALTER TABLE users ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE users ADD COLUMN custom_status TEXT;
ALTER TABLE users ADD COLUMN last_active_at TEXT NOT NULL DEFAULT '';
UPDATE users SET last_active_at = datetime('now') WHERE last_active_at = '';
