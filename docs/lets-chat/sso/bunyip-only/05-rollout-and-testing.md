# 05 - Rollout and testing

The operational sequence for the LC-22 cutover and the single path back if it
has to be undone.

## 5.1 Rollout

The cutover is forward-only: migration 0030 deletes rows, and the only way to
restore them is the pre-migration snapshot (§5.2). Run the rollout in this
order:

1. Announce the maintenance window; the local password / TOTP sign-in paths are
   going away permanently.
2. Confirm bunyip-api is reachable and its OIDC discovery / JWKS endpoints
   resolve from the lets-chat host - with the SSO flag on, the server refuses to
   start otherwise (§4.1).
3. **Take the snapshot.** This database snapshot, taken *before* migration 0030
   runs, is the only artifact that can restore the deleted local users (§5.2).
4. Deploy the cutover build and let migrations 0029 -> 0031 and 0033 apply:
   add `bunyip_sub` (0029), delete still-unlinked rows (0030), add the unique
   index (0031), erase legacy secrets (0033). See
   [`04-lets-chat-server-cutover.md`](04-lets-chat-server-cutover.md) §4.5.
5. Smoke-test "Sign in with Bunyip" end to end: a brand-new Bunyip identity
   provisions a fresh account, and an existing verified-email account is adopted
   onto its Bunyip subject (see [`03-account-provisioning.md`](03-account-provisioning.md) §3.1).
6. Verify a password login can no longer succeed (the `password_hash` sentinel
   from §3.2 / §4.2 fails every Argon2 verify).
7. **Re-graft returning users by hand.** Migration 0030 leaves the sessions,
   messages and memberships authored by deleted pre-cutover users as **orphan
   rows** (they are not cascaded away). When such a user returns through Bunyip
   they provision a *new* row with a *new* id, so their old content stays
   detached. If that history must follow them, the operator manually re-grafts
   the new identity onto the orphan data (re-point the old rows' author id at
   the new `users.id`). There is no automatic reconciliation; this is a
   deliberate, case-by-case operator action.

## 5.2 Rollback

There is no down-migration. Once 0030 has deleted the local users and 0033 has
erased the secrets, the database cannot be walked backward in place - the
deleted rows and secret values are simply gone.

The **only** rollback is to restore the snapshot taken in §5.1 step 3:

1. Stop the lets-chat server.
2. Restore the pre-0030 database snapshot over the live database.
3. Redeploy the pre-cutover build (the one that still serves the password
   path), since the restored schema predates the cutover.

This path stays viable for exactly one release: migration 0033 only *erases*
the legacy columns' contents, and the `0033_v2_drop_legacy_columns.sql` drop is
withheld until one release later (§4.5). After `_v2` ships, the columns are
gone and a snapshot from before it can no longer be restored onto the current
schema, so the rollback window is closed.

## 5.3 What is NOT reversible without the snapshot

- Local users deleted by 0030.
- Password hashes, TOTP secrets and email-verification state erased by 0033.
- Rows in the legacy token tables emptied by 0033.

Everything else (the additive `bunyip_sub` column, the unique index) is
ordinary forward schema and needs no special handling.
