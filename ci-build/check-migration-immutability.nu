#!/usr/bin/env nu

# LC-515: reject any change that modifies, renames, or deletes a migration
# file already committed on `main`.
#
# lets-chat applies its SQLite migrations at runtime via `sqlx::migrate!`
# (server/migrations/{auth,chat,settings}). sqlx records a checksum of each
# migration on first apply and re-verifies it on every start, so modifying,
# renaming, or deleting a migration that has already been applied to any
# database makes that database refuse to boot:
#
#   migration <version> was previously applied but has been modified
#
# Committed migrations are therefore immutable (internal/CLAUDE.md / LC-212): the only
# safe change is a NEW migration file with the next index. This gate fails any
# PR (and push to main) that Modifies (M), Renames (R), or Deletes (D) a
# migration relative to the merge-base with the base ref. Adding new migration
# files passes.
#
# This exact break took down the mokosh-server v0.4.0 production deploy on
# nc-01 (DEV-395). Review discipline alone has already failed, hence this gate.
#
# It fails loud (exit 2) if the diff itself cannot run, so a missing base ref
# or a shallow clone never silently reads as "nothing changed". In CI the
# checkout MUST use `fetch-depth: 0` so the merge-base with `origin/main` is
# available.
#
# Usage: nu ci-build/check-migration-immutability.nu [base_ref] [migrations_dir]
export def main [
    base_ref: string = "origin/main"              # merge-base ref to diff against
    migrations_dir: string = "server/migrations"  # migrations root to guard
] {
    # --diff-filter=MRD: only Modified, Renamed, Deleted paths (added files are
    # fine). The three-dot form diffs HEAD against the merge-base of base_ref
    # and HEAD, so unrelated commits already on the base do not register.
    let diff = (do {
        ^git diff --diff-filter=MRD --name-only $"($base_ref)...HEAD" -- $migrations_dir
    } | complete)

    if $diff.exit_code != 0 {
        print --stderr $"error: cannot diff '($migrations_dir)' against '($base_ref)...HEAD':"
        print --stderr $"  ($diff.stderr | str trim)"
        print --stderr "       The base ref must be fetched with full history (CI: fetch-depth: 0)."
        exit 2
    }

    let changed = ($diff.stdout | lines | where ($it | str trim | is-not-empty))

    if ($changed | is-empty) {
        print "migration immutability OK: no committed migration modified, renamed, or deleted"
        return
    }

    print --stderr "error: the following already-committed migration file(s) were modified, renamed, or deleted:"
    for f in $changed { print --stderr $"  - ($f)" }
    print --stderr ""
    print --stderr "Committed migrations are IMMUTABLE. sqlx checksums every applied migration and a"
    print --stderr "deployed database refuses to boot once the on-disk content disagrees with the"
    print --stderr "recorded checksum. Revert the change and add a NEW migration file (next index)"
    print --stderr "instead of editing, renaming, or deleting an existing one."
    exit 1
}
