# Bunyip-only SSO (LC-22 cutover)

This directory documents the LC-22 cutover that made federated identity from
bunyip-api (OIDC) the **sole** sign-in surface for lets-chat and retired the
local password / TOTP / email-verification paths.

Three already-shipped auth migrations cite documents in this tree:

| Migration | Cites |
| --- | --- |
| `server/migrations/auth/0029_bunyip_sub.sql` | [`03-account-provisioning.md`](03-account-provisioning.md) §3.4 |
| `server/migrations/auth/0030_purge_local_users.sql` | [`05-rollout-and-testing.md`](05-rollout-and-testing.md) §5.1 step 7, §5.2 |
| `server/migrations/auth/0033_null_legacy_auth_columns.sql` | [`04-lets-chat-server-cutover.md`](04-lets-chat-server-cutover.md) §4.5 |

Those migration files are immutable (per LC-212, `sqlx::migrate!` hashes the
whole file and refuses to start if a shipped migration's bytes change), so the
citations inside them can never be rewritten. This tree was never committed when
the migrations shipped, which left the three references dangling. LC-1105
restores the tree from the migration bodies and the live provisioning code
(`server/src/routes/bunyip_sso.rs`) so the paths resolve in a fresh clone.

The companion code-level notes for the same cutover live inline in
`server/src/oidc/` and `server/src/routes/bunyip_sso.rs` (LC-1102 replaced the
`.rs` citations of this tree with inline summaries rather than back-references).
