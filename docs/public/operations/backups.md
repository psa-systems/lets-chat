# Backups

Lets chat has a built-in admin backup and restore flow. The archive covers the three SQLite databases, the upload tree, and the avatar tree in one file.

## What is in a backup

- `auth.db`: sessions and SSO state (snapshot via `VACUUM INTO`, consistent at the time of request).
- `chat.db`: messages, rooms, enclaves, memberships.
- `files.db`: upload metadata.
- `uploads/`: user-uploaded files.
- `avatars/`: user avatars.
- `manifest.json`: one entry per included file with its size and SHA-256.

The archive is a plain zip. Encryption at rest is the operator's responsibility: encrypt the resulting file before storing it offsite, or use a storage backend that encrypts for you.

## Taking a backup

From `/admin/backup` an administrator starts a snapshot. The admin UI streams the archive as a download. For automation, hit the same path with an admin-scope API token; the response is an octet stream named `lets-chat-backup-<timestamp>.zip`.

Taking a backup does not stop the server. The `VACUUM INTO` and the upload tree walk are atomic enough that an in-flight message can be in the archive, but no message is partially written.

## Restoring

Restore is a two-step process that interrupts the server exactly once:

1. Admin uploads the archive to `/admin/backup/stage`. The server verifies every file against the manifest's SHA-256; a drift aborts with a named mismatch.
2. Admin confirms. The server extracts to a sibling staging directory and drops a marker file in the live data directory.
3. On the next startup, the server detects the marker, atomically swaps the staging directory into place, and keeps the previous data directory under a timestamped suffix (so you can roll back by moving directories).

An operator restart is required between step 2 and the live swap. A backup taken on a version newer than the running server is refused.

## Scheduling

Lets chat does not ship a backup scheduler. The admin API token path above makes it straightforward to call from your orchestrator's scheduled task runner: hit the backup endpoint on a cadence, write the output to object storage, enforce retention on the storage side.

## Testing restores

The only reliable test is actually restoring onto a spare host and signing in. Verify at minimum:

- Admin can sign in.
- A regular user can sign in (OIDC still binds to the users table).
- A recent message is readable in the room it was posted in.
- An uploaded file renders.

Do this once per quarter on real data, or whenever you change the storage path or the SSO configuration.
