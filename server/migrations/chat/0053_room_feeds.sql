-- LC-102: per-room read-only RSS/Atom + iCal feeds, gated by a per-feed token.
--
-- The token in the feed URL is the only credential (no session cookie). Only
-- the HMAC of the token is stored (mirrors incoming_webhooks; see
-- crate::auth::hash_api_token), so a DB leak does not expose live feed URLs.
-- A feed is bound to its creator (created_by): each fetch re-checks that user's
-- current read access to the room, so losing access (kick / room delete /
-- account removal) makes the feed start returning 410.
CREATE TABLE IF NOT EXISTS room_feeds (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id         INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    kind            TEXT    NOT NULL CHECK (kind IN ('rss', 'ical')),
    token_hash      TEXT    NOT NULL UNIQUE,
    created_by      TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    last_fetched_at TEXT,
    revoked_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_room_feeds_room ON room_feeds (room_id);
