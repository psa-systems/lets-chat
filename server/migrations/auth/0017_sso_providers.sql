-- Admin-managed OIDC providers. One row per IdP the deployment trusts.
-- Replaces the env-var single-provider model from L2 (which now acts as a
-- startup seed for backwards compatibility). Operators add / edit / disable
-- providers through /admin/sso. See docs/lets-chat/sso/10-admin-managed-providers.md.
CREATE TABLE IF NOT EXISTS sso_providers (
    -- Opaque admin-chosen slug. URL-safe. Used in /auth/sso/:id/start and
    -- /auth/sso/:id/callback. Cannot change after creation (would orphan
    -- existing sso_identities/sso_flows rows that reference the issuer).
    id                       TEXT PRIMARY KEY,
    -- Discriminator. Today only 'oidc'; SAML is filed as a follow-up
    -- per docs/lets-chat/sso/00-overview.md non-goals.
    kind                     TEXT NOT NULL CHECK (kind IN ('oidc')),
    -- Label rendered on the login button.
    display_name             TEXT NOT NULL,
    -- Discovery URL prefix; the RP fetches {issuer}/.well-known/openid-configuration.
    issuer_url               TEXT NOT NULL,
    client_id                TEXT NOT NULL,
    -- AES-256-GCM ciphertext using LETS_CHAT_SECRET_KEY, same wrapper that
    -- protects the SMTP password. When LETS_CHAT_SECRET_KEY is unset, the
    -- admin-create route refuses with 503; provider rows therefore only
    -- exist on deployments with the key configured.
    client_secret_encrypted  BLOB NOT NULL,
    -- Space-separated OIDC scopes. Default covers openid + email + profile
    -- which is what most IdPs need; operators add 'groups' / 'offline_access'
    -- as required.
    scopes                   TEXT NOT NULL DEFAULT 'openid email profile',
    -- JSON object mapping logical claim names (email, name, username,
    -- groups, email_verified) to the IdP's actual claim names. Empty
    -- object means "use defaults" (email -> "email", name -> "name", etc.).
    -- See doc 10 section 2 for the shape.
    attribute_map_json       TEXT NOT NULL DEFAULT '{}',
    -- Per-provider equivalent of the old LETS_CHAT_SSO_AUTOPROVISION env
    -- var. 1 = a successful SSO sign-in with a never-before-seen email
    -- creates a fresh user. 0 = refuse and show "ask your admin".
    allow_signup             INTEGER NOT NULL DEFAULT 0,
    -- Per-provider opt-in to doc 02 section 2's "auto-link on
    -- email_verified=true" path. 0 forces every email collision through
    -- the link-required interstitial instead.
    auto_link_verified_email INTEGER NOT NULL DEFAULT 1,
    -- Audit trail timestamps. A live provider has enabled_at IS NOT NULL
    -- AND (disabled_at IS NULL OR disabled_at < enabled_at). Toggling
    -- writes a fresh timestamp into one column without erasing the other.
    enabled_at               INTEGER,
    disabled_at              INTEGER,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
);

-- One row per issuer. Two rows pointing at the same IdP would race on
-- (issuer, subject) writes into sso_identities.
CREATE UNIQUE INDEX IF NOT EXISTS idx_sso_providers_issuer ON sso_providers(issuer_url);
