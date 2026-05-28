-- LC-78-AVATAR-PROXY: cache of proxied foreign bridge-avatar fetches.
--
-- The LC-78 v1 design rejected non-null foreign_avatar on bridge messages
-- because hot-linking a foreign Matrix homeserver's avatar URL from rendered
-- HTML leaks the viewer's IP + User-Agent + Referer to that homeserver on
-- every render. This table backs the v2 proxy: lets-chat fetches the foreign
-- bytes once, caches them on disk, and serves them from a same-origin URL
-- (/media/bridge-avatar-proxy/{hash}). Every viewer-side fetch hits lets-chat
-- only; the foreign homeserver sees at most one fetch from the server.
--
-- The structural side-channel closure (vs storing the foreign URL on the
-- message row directly) is that the URL never appears in rendered HTML; only
-- the opaque hash does. Reading the HTML reveals nothing about which
-- homeservers the room bridges with.
--
-- Cache key is sha256 of the canonical foreign URL (lowercased scheme + host,
-- default ports stripped, fragment stripped). The same foreign URL across
-- many messages dedupes to one cache row + one disk file.
--
-- fetch_status state machine:
--   'pending' -> 'ok'      after a successful fetch + sniff + re-encode.
--   'pending' -> 'failed'  on any of: SSRF re-check denial, HTTP error,
--                          size-cap hit, magic-byte mismatch, decoder
--                          rejection, disk write error.
--   'failed'  is terminal in v2. The render falls back to initials via the
--             <img onerror> path; a future LC-78-AVATAR-REFRESH could add a
--             retry surface. v2 chose render-initials-forever over outbound
--             retry traffic to dead homeservers.
--
-- last_seen_at is bumped on every bridge-message POST that references the
-- hash. The GC sweep deletes rows whose last_seen_at is older than the
-- retention threshold AND that no live messages.bridge_foreign_avatar row
-- still points at. Disk file is deleted in the same sweep.
CREATE TABLE IF NOT EXISTS bridge_avatar_proxies (
    hash           TEXT PRIMARY KEY,
    foreign_url    TEXT NOT NULL UNIQUE,
    content_type   TEXT,
    byte_size      INTEGER,
    fetched_at     TEXT,
    last_seen_at   TEXT NOT NULL DEFAULT (datetime('now')),
    fetch_status   TEXT NOT NULL DEFAULT 'pending',
    failure_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_bridge_avatar_proxies_status
    ON bridge_avatar_proxies (fetch_status, last_seen_at);
