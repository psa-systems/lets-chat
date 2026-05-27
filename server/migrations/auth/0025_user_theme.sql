-- LC-191: per-user UI theme preference. NULL or 'system' = follow the OS
-- (prefers-color-scheme); 'light' / 'dark' force a theme. Mirrors users.locale
-- (LC-100). Resolved no-flash on the client from the `lc-theme` cookie, which
-- the locale middleware keeps in sync with this column.
ALTER TABLE users ADD COLUMN theme TEXT;
