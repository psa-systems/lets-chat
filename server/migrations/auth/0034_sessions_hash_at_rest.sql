-- LC-514: hash session tokens at rest.
--
-- The `sessions.id` column held the RAW session cookie value (a 64-char
-- alphanumeric token minted via OsRng). A read-only DB compromise (a
-- backup leak, a replica with read access, an ops-shell SELECT) handed
-- the attacker live session cookies they could paste straight into the
-- `session=` cookie header and impersonate users without ever cracking
-- a password.
--
-- The cookie value itself is a 256-bit opaque random token; storing the
-- plaintext as the lookup key treats the DB as a secret-keeping vault,
-- which it isn't. Hash it instead.
--
-- The lookup logic at `server/src/db/auth.rs` is updated in lockstep:
-- the cookie still carries the plaintext, but every lookup hashes the
-- presented value via SHA-256 before the WHERE clause. Existing rows
-- have their `id` updated in place to the SHA-256 of the original token
-- value here, so the lookup against the hashed key matches and no user
-- is forced through a re-login.
--
-- SQLite ships SHA-256 only behind an extension (`sqlcipher` etc.) we
-- don't enable, so this migration cannot UPDATE in pure SQL - the
-- in-place re-hash is performed by `server/src/db/sessions_migrate.rs`
-- on the next startup after this migration applies. It is idempotent:
-- a row whose `id` is already 64 hex chars (the SHA-256 hex shape) is
-- left alone. Once it has run, the table holds only hashes; the next
-- attacker with a backup gets no usable session.

CREATE TABLE IF NOT EXISTS sessions_hash_migration_marker (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    completed   INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO sessions_hash_migration_marker (id, completed) VALUES (1, 0);
