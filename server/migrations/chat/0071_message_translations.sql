-- LC-486: cached per-message LLM translations. Keyed by (message_id, locale)
-- so viewers sharing a locale reuse one translation. CASCADE on message delete;
-- the edit path deletes rows for a message so a re-translation reflects the new
-- body. `translated` is the model output (markdown), rendered through the
-- message markdown pipeline on display.
CREATE TABLE IF NOT EXISTS message_translations (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    locale      TEXT    NOT NULL,
    translated  TEXT    NOT NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, locale)
);
