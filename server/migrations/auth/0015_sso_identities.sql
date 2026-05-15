-- SSO support: external-identity links + in-flight OIDC flow state.
--
-- One row per (user, IdP-identity) link. Provider is parameterized
-- via `issuer` so multi-IdP support (BYO-SSO) can land later without
-- a schema bump. Today every row's `issuer` is the configured
-- mokosh-server URL.
--
-- See docs/lets-chat/sso/05-schema-and-account-linking.md.

CREATE TABLE sso_identities (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    issuer        TEXT NOT NULL,
    subject       TEXT NOT NULL,
    -- Last-known email from the id_token. Metadata only; the
    -- (issuer, subject) pair is the actual identity key.
    email         TEXT,
    -- True when the linking happened via the email_verified=true
    -- auto-link path (vs. an explicit password-confirmed link or
    -- in-settings link). Audit + UI badge use this.
    auto_linked   INTEGER NOT NULL DEFAULT 0,
    linked_at     TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (issuer, subject)
);

CREATE INDEX idx_sso_identities_user ON sso_identities(user_id);

-- Server-side state for in-flight OIDC flows. `flow_id` is what's in
-- the OIDC `state` parameter sent to mokosh-server; the real CSRF
-- state, PKCE verifier, nonce, return-to URL, and link-vs-signin
-- intent live in the row. One-shot use: the callback handler
-- consumes (and deletes) the row in one transaction.
CREATE TABLE sso_flows (
    flow_id       TEXT PRIMARY KEY,
    csrf_state    TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    return_to     TEXT NOT NULL,
    -- 'sign_in' | 'link'. The kind drives the callback handler's
    -- branch (link branch attaches the new identity to an already-
    -- signed-in user; sign_in branch is the public sign-in flow).
    kind          TEXT NOT NULL,
    -- Non-null iff kind = 'link'. The signed-in user this flow
    -- is attaching the new identity to.
    user_id       TEXT REFERENCES users(id) ON DELETE CASCADE,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    -- 10-minute window. Enforced by `consume_sso_flow` on lookup;
    -- expired rows are pruned by a background task.
    expires_at    TEXT NOT NULL
);

CREATE INDEX idx_sso_flows_expires_at ON sso_flows(expires_at);

-- Relax `users.password_hash` from NOT NULL to nullable so SSO-only
-- users (auto-provisioned via the SSO callback) can exist without a
-- local password. Existing rows keep their hash; the application
-- layer's password sign-in path already treats NULL/empty hashes as
-- "no password set" (see server/src/db/auth.rs).
--
-- SQLite has no in-place ALTER COLUMN DROP NOT NULL; the canonical
-- workaround is add new column, copy, drop old, rename. The data
-- preserved is: password_hash. Every other column on users is left
-- alone via "ALTER TABLE ... DROP COLUMN" + "ADD COLUMN" not being
-- needed - we only touch the one column.
ALTER TABLE users ADD COLUMN password_hash_nullable TEXT;
UPDATE users SET password_hash_nullable = password_hash;
ALTER TABLE users DROP COLUMN password_hash;
ALTER TABLE users RENAME COLUMN password_hash_nullable TO password_hash;
