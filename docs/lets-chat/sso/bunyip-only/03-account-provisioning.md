# 03 - Account provisioning

How a `users` row is resolved or created for a verified Bunyip identity after
the LC-22 cutover, and how that identity is stored.

The authoritative implementation is `resolve_or_provision_user` and
`create_user_from_bunyip` in `server/src/routes/bunyip_sso.rs`; this document
records the decisions those functions encode and the storage model migrations
0029 / 0030 / 0031 / 0043 establish.

## 3.1 Resolve-or-provision decision tree

The `/auth/bunyip/callback` handler, once it has a verified `id_token`, maps the
token's `sub` (the stable Bunyip subject) to a local `users` row:

1. A row already **linked** to this `bunyip_sub` is reused outright, subject to
   the banned / bot checks (a banned or bot row is refused, never signed into).
2. Otherwise, a row that owns the same **verified** email is **adopted** only if
   it is unlinked, non-bot and non-banned. Adoption writes this `bunyip_sub`
   onto that row.
3. A row whose email is present but **unverified** is released (not adopted) so
   the email can be claimed cleanly by whoever verifies it through Bunyip.
4. A row already linked to a **different or rotated** `bunyip_sub` is refused as
   an identity conflict rather than silently relinked (LC-618 relinked a rotated
   subject once; that is now a hard conflict).
5. Failing all of the above, a fresh account is provisioned by
   `create_user_from_bunyip`.

## 3.2 Fresh-account provisioning

`create_user_from_bunyip` writes a new `users` row anchored to the Bunyip
subject. It has no password to store, so `password_hash` is set to the empty
string: the same cutover-safe sentinel migration 0033 writes for surviving
pre-cutover rows (an Argon2 verify against an empty hash always fails, so the
sentinel can never authenticate a password login). TOTP and
email-verification columns are left at their non-secret defaults.

## 3.3 Unlinked rows

An **unlinked** account is one whose `bunyip_sub` is the empty string. Unlinking
is how an operator frees an identity (for example, to resolve a rotated-subject
conflict from §3.1 step 4 by unlinking the stale row and letting the real
subject adopt or provision cleanly). More than one row may sit unlinked at the
same time - see §3.4.

## 3.4 Storage rationale and the hard-cut sequence

Federated identity from bunyip-api is the sole sign-in surface post-cutover, so
**every** `users` row is anchored to a Bunyip identity and there is no nullable
case. The storage model is therefore a plain `TEXT NOT NULL` column rather than
the additive design's nullable column plus a partial unique index.

SQLite does not allow `ADD COLUMN ... UNIQUE` in a single statement, so the
column and its uniqueness constraint land in a staged sequence:

| Step | Migration | Effect |
| --- | --- | --- |
| 1 | `0029_bunyip_sub.sql` | `ALTER TABLE users ADD COLUMN bunyip_sub TEXT NOT NULL DEFAULT ''`. Every existing row gets the empty-string default (i.e. starts **unlinked**). |
| 2 | `0030_purge_local_users.sql` | `DELETE FROM users WHERE bunyip_sub = ''` - the hard cut (see [`05-rollout-and-testing.md`](05-rollout-and-testing.md) §5.1). After this, every **surviving** row carries a real subject. |
| 3 | `0031_bunyip_sub_unique.sql` | `CREATE UNIQUE INDEX users_bunyip_sub_unique ON users(bunyip_sub)`. Safe now that no row holds the empty-string default, so uniqueness is satisfied. |

The `DEFAULT ''` is what makes the empty string mean "unlinked" everywhere in
the code (§3.3).

### 3.4.1 Follow-up: multiple unlinked rows (LC-698)

The 0031 index was **unconditional**, which made the empty string itself unique:
at most one row could be unlinked, so an admin could not unlink a second user to
recover from a rotated-subject conflict. Migration
`0043_bunyip_sub_unique_partial.sql` re-creates it as a **partial** index
(`WHERE bunyip_sub <> ''`), so uniqueness still binds every real subject while
any number of rows may sit unlinked. This is the index in force today.
