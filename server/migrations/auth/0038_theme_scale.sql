-- LC-569: UI scaling axis. Adds `theme_scale` alongside the existing
-- theme_mode / theme_palette pair. NULL = "default" (the current size).
-- Values: "compact" | "default" | "large" | "xl". Applied as a root rem scale
-- (html[data-scale]) so every rem-based spacing/type step scales with it.
ALTER TABLE users ADD COLUMN theme_scale TEXT;
