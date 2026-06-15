-- LC-312: saved searches. A user can save a message-search query string (free
-- text plus LC-280 operators like from:/before:/after:) and re-run it with one
-- click from the sidebar search popover. The query string is also its display
-- label (no separate name), so it is self-describing for operator queries. The
-- per-user cap is enforced on the write path, not in the schema.
CREATE TABLE saved_searches (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     TEXT NOT NULL,
    query       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, query)
);

CREATE INDEX idx_saved_searches_user ON saved_searches (user_id);
