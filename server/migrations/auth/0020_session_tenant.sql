-- Per-session active-tenant scope from the IdP's `mokosh_active_tenant`
-- claim. NULL for password sign-ins (no IdP claim available) and for
-- SSO sign-ins from providers that don't emit the claim (BYO-SSO IdPs
-- typically don't). Per doc 02 section 16.
--
-- The chat-side query layer's `WHERE tenant_id = ?` consumers are out
-- of scope for this phase; this column just makes the value available
-- so they can wire it up independently.
ALTER TABLE sessions ADD COLUMN tenant_id TEXT;
