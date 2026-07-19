-- LC-587: suspicious-login notify-and-approve gate (adapts mokosh PMS-658 to
-- lets-chat's pure-RP SSO callback + SQLite).
--
-- A login that has already authenticated at the Bunyip callback but looks
-- suspicious (new country - reusing the LC-580 signal - and/or a new device)
-- does NOT mint a session. Instead one `login_approvals` row is inserted and a
-- single-use 6-digit code is emailed; the user re-submits the code on an
-- interstitial page to complete the login (mirrors an MFA re-submit). Only the
-- SHA-256 hash of the code is stored, so a leaked row cannot be replayed. Rows
-- are short-lived (expires_at) and single-use (consumed_at).
--
-- Single-tenant per deployment (SQLite), so no tenant_id / RLS (unlike the
-- multi-tenant PMS-658 Postgres tables).

CREATE TABLE login_approvals (
    -- Opaque single-use token, also the interstitial form's lookup key. The
    -- emailed 6-digit code (hashed below) is the second factor; the token alone
    -- grants nothing.
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- SHA-256 hex of the emailed 6-digit code; the code itself is never stored.
    code_hash TEXT NOT NULL,
    -- Context of the flagged attempt, applied as the new baseline on approval.
    country TEXT,
    device_hash TEXT,
    ip TEXT,
    user_agent TEXT,
    -- Wrong-code attempts against this challenge; capped by the service.
    attempts INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_login_approvals_user ON login_approvals(user_id);
CREATE INDEX idx_login_approvals_expires ON login_approvals(expires_at);

-- Per-user set of known login devices, identified by the SHA-256 hash of a
-- first-party `device_id` cookie lets-chat sets on a cleared login. A login
-- from a device_hash not in this set is a "new device" signal, but only once
-- the user already has >= 1 known device (the first device is baseline). A
-- client that presents no device_id contributes no device signal, so the gate
-- degrades to country-only (fail-open, no over-gating).
CREATE TABLE user_login_devices (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_hash TEXT NOT NULL,
    user_agent TEXT,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, device_hash)
);

CREATE INDEX idx_user_login_devices_user ON user_login_devices(user_id);
