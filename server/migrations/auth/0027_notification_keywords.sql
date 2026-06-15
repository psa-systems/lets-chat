-- LC-304: per-user highlight words. A posted/edited message containing one of
-- a user's words triggers the same treatment as an @mention (badge, row
-- highlight, inbox, notification) by writing a normal chat.db::mentions row at
-- write time; this table only stores the words to match against.
--
-- Words are stored as entered (trimmed) and matched case-insensitively with
-- word boundaries by routes::room::append_keyword_targets. The per-user cap
-- (25 words, 50 chars each) is enforced on the write path, not in the schema.
CREATE TABLE notification_keywords (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     TEXT NOT NULL,
    word        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, word)
);

CREATE INDEX idx_notification_keywords_user ON notification_keywords (user_id);
