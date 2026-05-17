-- Map IdP-emitted group claim values to lets-chat enclave memberships.
-- Reconciled at SSO sign-in time (L17): users gain / lose / get re-roled
-- in the listed enclaves based on the groups claim from the id_token.
--
-- enclave_id references the `enclaves` table which lives in the chat
-- database; cross-database foreign keys aren't enforceable in SQLite,
-- so the application layer handles consistency (admin UI populates the
-- dropdown from chat.enclaves; the L17 sync helper skips any mapping
-- whose enclave_id no longer resolves).
--
-- See docs/lets-chat/sso/10-admin-managed-providers.md section 3.
CREATE TABLE IF NOT EXISTS sso_group_mappings (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Cascades on provider delete so a removed provider's mappings
    -- don't linger as orphans the admin UI would have to clean up.
    provider_id   TEXT NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
    -- Exact-match value from the IdP's groups-claim array. Regex /
    -- prefix matching is deferred per doc 10 section 3.
    group_value   TEXT NOT NULL,
    -- Integer FK into the chat-DB `enclaves` table. Not enforced by
    -- SQLite (different database); the admin UI restricts inputs to
    -- enclaves that currently exist.
    enclave_id    INTEGER NOT NULL,
    -- Role to grant inside that enclave when the group is present.
    -- Constrained to the three roles the chat-side membership table
    -- recognises.
    role          TEXT NOT NULL DEFAULT 'User'
                  CHECK (role IN ('User', 'Moderator', 'Admin')),
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    -- Same (provider, group_value, enclave_id) triple can't appear
    -- twice: an operator either wants a group to grant access to an
    -- enclave or not. The role is the variable, but two rows with
    -- different roles for the same triple would be ambiguous.
    UNIQUE (provider_id, group_value, enclave_id)
);

-- Sign-in path looks up by (provider_id, group_value) when projecting
-- the id_token's groups claim onto enclave membership. Without this
-- index every sign-in scans the full table.
CREATE INDEX IF NOT EXISTS idx_sso_group_mappings_lookup
    ON sso_group_mappings(provider_id, group_value);
