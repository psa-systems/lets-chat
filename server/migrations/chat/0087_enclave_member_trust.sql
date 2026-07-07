-- LC-551: graduated member trust. A member joins an enclave as 'new' and
-- graduates to 'trusted' after posting enough (see db::enclave graduation). New
-- members are held to a minimum interval between posts, blunting drive-by spam
-- from a freshly-joined account without throttling established members. Owners
-- and admins are always treated as trusted regardless of this column.
ALTER TABLE enclave_members
    ADD COLUMN trust TEXT NOT NULL DEFAULT 'new'
    CHECK (trust IN ('new', 'trusted'));

-- Everyone who is ALREADY a member predates the feature and is an established
-- participant; mark them trusted so enabling this never suddenly throttles the
-- existing community. Only members who join after this migration start as 'new'.
UPDATE enclave_members SET trust = 'trusted';
