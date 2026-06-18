-- LC-22: erase password + TOTP + email-verification secrets in the live
-- database. The columns themselves stay - per LC-212 (migration immutability)
-- the original column-define migrations cannot be edited, so a clean drop
-- waits for a v2 follow-up (`0033_v2_drop_legacy_columns.sql`) one release
-- after cutover so the snapshot-restore rollback path remains viable for one
-- release. See docs/lets-chat/sso/bunyip-only/04-lets-chat-server-cutover.md
-- (§4.5 migrations table) for the two-release sequence.
--
-- `password_hash` is NOT NULL in the 0001 schema; can't be made NULL without
-- a table rewrite. Setting it to the empty string is the cutover-safe sentinel
-- (Argon2 verify on an empty hash always fails). `create_user_from_bunyip`
-- writes the same sentinel for fresh SSO users.
--
-- Surviving rows (those Bunyip auto-provisioned post-cutover) had no
-- password / TOTP data in the first place, so this is effectively a no-op
-- for them. The UPDATE exists for pre-cutover defense in depth, in case a
-- snapshot restore brings rows back without re-running 0030.
UPDATE users
SET password_hash             = '',
    totp_secret_encrypted     = NULL,
    totp_nonce                = NULL,
    totp_recovery_hashes      = NULL,
    totp_enabled              = 0,
    email_verified_at         = NULL;

DELETE FROM password_reset_tokens;
DELETE FROM email_verification_tokens;
DELETE FROM pending_2fa;
DELETE FROM login_alert_devices;
DELETE FROM pending_registrations;
