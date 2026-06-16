-- LC-319: auto-expiring custom status. Nullable ISO datetime; NULL means the
-- custom status never expires. Cleared (set to NULL) whenever the custom text
-- is cleared, and swept by spawn_status_expiry_scanner once it passes. This
-- column is deliberately NOT added to any existing SELECT list, so the User /
-- UserRecord model and its read sites are untouched.
ALTER TABLE users ADD COLUMN custom_status_expires_at TEXT;
