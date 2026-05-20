-- LC-72: scoped personal API tokens. The plaintext token is shown to the
-- user exactly once at creation; only an HMAC-SHA256 of it (keyed by the
-- server secret) is stored, so a DB leak cannot reconstruct usable tokens.
--
-- scopes is a space-separated OAuth2-style list (e.g. "messages:read
-- messages:write rooms:read"). A request is refused 403 when the token is
-- valid but lacks the route's required scope; 401 when the token is
-- missing, expired, or revoked.
--
-- Rows are retained after revoke/expiry for audit (a future retention sweep
-- removes them); revoked_at / expires_at gate use without deleting the row.
CREATE TABLE IF NOT EXISTS api_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    scopes       TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT,
    expires_at   TEXT,
    revoked_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON api_tokens (user_id, created_at DESC);
