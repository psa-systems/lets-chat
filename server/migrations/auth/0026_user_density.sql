-- LC-194: per-user UI density. NULL or 'comfortable' = the default roomy
-- spacing; 'compact' tightens message rows and lists. Resolved no-flash on the
-- client from the `lc-density` cookie, which the locale middleware keeps in
-- sync with this column (mirrors users.theme, LC-191).
ALTER TABLE users ADD COLUMN density TEXT;
