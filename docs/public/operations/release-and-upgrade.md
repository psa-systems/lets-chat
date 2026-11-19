# Release and upgrade

How lets chat is released, and how an operator moves a running deployment from one version to the next.

## Release cadence

Lets chat ships two artifacts for each release:
- A tagged container image at `dev.a8n.run/psa-systems-private/lets-chat:vX.Y.Z` (and `:latest` on a merge to main).
- Desktop binaries per platform, published to the operator's own download host (the signed release manifest the desktop update check reads).

A release note landing on `CHANGELOG.md` names every operator-visible behavior change, every new environment variable, every migration that requires attention, and every security fix. Read it before rolling an upgrade.

## Pinning a version

In production, pin an exact image tag. `latest` is for staging and demos:

```
image: dev.a8n.run/psa-systems-private/lets-chat:v0.14.0
```

A pinned tag is immutable. Rolling forward is a tag change and a container restart.

## Upgrading the server

1. Read the changelog entry for the target version.
2. Take a backup (see [Backups](backups.md)).
3. Update the image tag and apply.
4. Watch the first boot. Database migrations run on startup; if any fail the server aborts and leaves the data directory untouched.
5. Hit `GET /healthz` to confirm the new version is up.

Migrations are forward-only. Rolling back means rolling back the image AND restoring a pre-upgrade backup, because a newer schema in the data directory cannot be read by an older binary.

## Migration behavior

Lets chat uses SQLx migrations against three separate SQLite files. Migrations are idempotent and committed on main are immutable once shipped. A migration that fails prints the migration name and the SQL error; the server does not start until the fault is fixed.

If a migration fails because of an unexpected database state (manual schema change, botched restore), fix it offline (SQLite CLI, backup, restore). Do not edit a migration file to work around it.

## Upgrading desktop clients

The desktop app checks the operator's update host at startup and once per day. See [Desktop update signing](desktop-update-signing.md) for publishing a signed release manifest.

Staff can force an immediate check from the About screen. Admins can require an up-to-date desktop by setting a minimum version in `/admin/desktop-minimum-version`; clients below the floor get a sign-in refusal with a link to the update.

## Rollback

Rollback paths:

- If the new version booted and runs but has a bug: restart the container with the previous image tag. Keep data as-is.
- If the new version booted and ran migrations and you need to revert the schema: pull the pre-upgrade backup, stage it, confirm, restart. The live data directory and the staging directory follow the dance described in [Backups](backups.md).

Rolling back is a real operation. Do it on a timer and with a backup. "Just roll back the image" works only when no migration ran.
