-- LC-22: enforce the UNIQUE constraint on `users.bunyip_sub` introduced in
-- 0029. SQLite cannot do `ADD COLUMN ... UNIQUE` inline, so the index lands
-- here as a separate statement after 0030 deletes every row with the
-- empty-string default. See 0029 + 0030 for the staged sequence.
CREATE UNIQUE INDEX users_bunyip_sub_unique ON users(bunyip_sub);
