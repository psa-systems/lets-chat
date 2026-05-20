-- LC-96: branding (logo + colors + login text), per scope.
--
-- Scope is composite (kind, id):
--   ('global', 0)         the deployment-wide branding (standalone +
--                         fallback for SaaS enclaves with no row)
--   ('enclave', N)        per-enclave branding (SaaS); N is enclaves.id
--
-- enclave_id = 0 is impossible (AUTOINCREMENT starts at 1) so the
-- sentinel never collides with a real enclave. Per-enclave rows fall
-- back to ('global', 0) at resolution time.
-- `logo_upload_id` is intentionally NOT a FOREIGN KEY: every test
-- pool with a hand-rolled migration list would otherwise need to
-- include `0012_uploads.sql` upstream just to satisfy the reference,
-- which would explode the backfill surface. The application-level
-- invariant (only IDs returned from a successful `insert_upload`
-- ever land here, and the orphan sweep exempts whatever
-- `branding.logo_upload_id` points at via a subquery) carries the
-- same guarantee.
CREATE TABLE IF NOT EXISTS branding (
    scope_kind     TEXT NOT NULL CHECK (scope_kind IN ('global', 'enclave')),
    scope_id       INTEGER NOT NULL,
    logo_upload_id INTEGER,
    primary_color  TEXT NOT NULL DEFAULT '#2563eb',
    accent_color   TEXT NOT NULL DEFAULT '#1d4ed8',
    login_heading  TEXT NOT NULL DEFAULT '',
    login_body     TEXT NOT NULL DEFAULT '',
    updated_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_by     TEXT NOT NULL DEFAULT 'system',
    PRIMARY KEY (scope_kind, scope_id)
);

-- Seed the global default so resolution never returns None for the
-- global scope; per-enclave rows are created on first save.
INSERT OR IGNORE INTO branding (scope_kind, scope_id, updated_by)
VALUES ('global', 0, 'system');
