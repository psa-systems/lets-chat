# 04 - lets-chat-server cutover

The server-side changes that turn Bunyip SSO from an additive option into the
sole sign-in surface, and the order they are applied in the live database.

## 4.1 Posture

Post-cutover there is no local-password branch to fall back to. With the SSO
flag on, the binary refuses to start if bunyip-api's OIDC discovery / JWKS
fetches fail: an all-or-nothing posture with no degraded "not configured" mode
once the process is up. (This is the posture summarised inline in
`server/src/oidc/mod.rs`.)

## 4.2 Legacy columns stay, secrets are erased

The local-auth **columns** (`password_hash`, the `totp_*` family,
`email_verified_at`, and the legacy token tables) are not dropped at cutover.
Per LC-212 the original column-defining migrations cannot be edited, and a clean
`DROP COLUMN` is deferred so that a snapshot-restore rollback (§4.5,
[`05-rollout-and-testing.md`](05-rollout-and-testing.md) §5.2) stays viable for
one release. Instead, migration 0033 **erases the secret values** in place:

- `password_hash` is `NOT NULL` in the 0001 schema and cannot be nulled without
  a table rewrite, so it is set to the empty-string sentinel (§3.2 of
  [`03-account-provisioning.md`](03-account-provisioning.md)).
- `totp_*` and `email_verified_at` are nulled / zeroed.
- The legacy token / pending tables (`password_reset_tokens`,
  `email_verification_tokens`, `pending_2fa`, `login_alert_devices`,
  `pending_registrations`) are emptied.

For rows Bunyip auto-provisioned after cutover this is effectively a no-op (they
never held password or TOTP data); the UPDATE exists as pre-cutover defense in
depth, in case a snapshot restore brings rows back without re-running 0030.

## 4.3 Session issuance is unchanged

The SSO callback issues the lets-chat session cookie via the **same** helpers a
password login used to, so everything downstream of "who is signed in" is
untouched by the cutover.

## 4.4 Identity resolution

See [`03-account-provisioning.md`](03-account-provisioning.md) for the
resolve-or-provision decision tree the callback applies.

## 4.5 Migrations table (two-release sequence)

The cutover is spread across two releases so the rollback window in §5.2 stays
open for one release after the secrets are erased.

| Migration | Release | Effect | Reversible via snapshot? |
| --- | --- | --- | --- |
| `0029_bunyip_sub.sql` | cutover | Add `bunyip_sub TEXT NOT NULL DEFAULT ''`. | yes |
| `0030_purge_local_users.sql` | cutover | Delete every still-unlinked (empty-`bunyip_sub`) row - the hard cut. | only from the pre-0030 snapshot (§5.2) |
| `0031_bunyip_sub_unique.sql` | cutover | Add the unique index on `bunyip_sub`. Superseded by `0043` below. | yes |
| `0033_null_legacy_auth_columns.sql` | cutover | Erase password / TOTP / email-verification secrets; empty the legacy token tables. Columns remain. | only from the pre-cutover snapshot (§5.2) |
| `0033_v2_drop_legacy_columns.sql` | cutover **+ 1** | Drop the now-empty legacy columns for good. **Not** written until one release after cutover, so the snapshot-restore path above is still meaningful for that release. | n/a (terminal) |
| `0043_bunyip_sub_unique_partial.sql` | post-cutover, LC-698 | Narrows the `0031` unique index to `WHERE bunyip_sub <> ''`, allowing more than one unlinked row. See §3.4.1 of [`03-account-provisioning.md`](03-account-provisioning.md). | yes |

The `_v2` follow-up is deliberately a separate, later migration rather than an
edit to 0033: editing a shipped migration would change its checksum and make
`sqlx::migrate!` panic at startup on every database that already recorded the
old one (LC-212).
