-- LC-698: allow more than one UNLINKED row.
--
-- `bunyip_sub` is NOT NULL DEFAULT '' (0029), so an unlinked account is one
-- holding the empty string. The 0031 index was unconditional, which made ''
-- itself unique: at most one account could sit unlinked, and an admin could not
-- unlink a second user to recover it from a rotated-sub identity conflict.
-- Re-create it as a partial index so uniqueness still binds every REAL subject
-- while any number of rows may be unlinked.
DROP INDEX IF EXISTS users_bunyip_sub_unique;
CREATE UNIQUE INDEX users_bunyip_sub_unique ON users(bunyip_sub) WHERE bunyip_sub <> '';
