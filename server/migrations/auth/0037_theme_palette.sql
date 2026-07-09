-- LC-541 P1: split the single UI theme pref into mode + palette.
-- `theme` held the light/dark/hc-light/hc-dark mode; rename it to `theme_mode`.
-- Add `theme_palette` (NULL = blue-harbor, the current look). SQLite 3.25+
-- supports RENAME COLUMN; data is preserved untouched.
ALTER TABLE users RENAME COLUMN theme TO theme_mode;
ALTER TABLE users ADD COLUMN theme_palette TEXT;
