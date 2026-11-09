# LC-1008 runbook: purge stale `refs/pull/*` so the LC-522 secrets stop being served

**Audience:** whoever administers the `dev.a8n.run` Forgejo host. Every step runs
**on the server, in the on-disk repository**, as the Forgejo system user. None of
this can be done by a `git push` or the API: `refs/pull/*` are server-managed, and
`git gc` needs filesystem access.

## Why

The LC-522 history rewrite (force-pushed 2026-09-21 ~19:31) rewrote
`refs/heads/*` and `refs/tags/*` but, by design, left `refs/pull/*` alone
(clobbering them destroys the diff view on every past PR). Forgejo re-syncs
`refs/pull/<n>/head` only while a PR is **open**, so every **closed** PR's ref is
frozen on a pre-rewrite commit and keeps the scrubbed objects addressable.

**Confirmed still live on 2026-11-09** from a read-only clone:

- `git ls-remote origin 'refs/pull/*/head'` → **1088** refs (`refs/pull/*/merge`: 0).
- `git fetch origin 'refs/pull/253/head:...'` then `git show 875b7433^:compose.dev.yml.1`
  still serves the pre-redaction file, which contains real `LETS_CHAT_SECRET_KEY`
  and `LETS_CHAT_SMTP_PASSWORD` values (PR #253 is itself the SMTP-redaction PR).
- `git fetch origin 875b7433` (by SHA) is already refused by the server
  (`uploadpack.allowAnySHA1InWant` is off), so today the exposure is reachable
  **only via the frozen pull refs** — which is exactly what this runbook removes.

## Prerequisite that actually ends the exposure (do not skip)

This runbook stops **the server** from serving the old values. It does **not**
un-expose them: anyone who cloned before 2026-09-21 already holds them. The
`LETS_CHAT_SECRET_KEY` and `LETS_CHAT_SMTP_PASSWORD` (and any other secret in the
scrubbed blobs) **must be rotated** — that is LC-522 AC1 and the step that ends
the credential's usefulness. Treat rotation as the real fix and this as closing
the "the server still hands them out" finding.

## Accepted tradeoff

Deleting a closed PR's `refs/pull/<n>/head` removes that PR's **diff view** in the
web UI (the PR record, comments, reviews and CI history stay). Per the ticket this
is acceptable for closed PRs, given the alternative is leaving live credentials
fetchable. Open PRs are **not** affected: their refs are left in place, and even
if one were removed Forgejo re-creates it on the next sync.

## Preconditions

- Shell on `dev.a8n.run` as the Forgejo system user (the account that owns the
  repo data, often `git` or `forgejo`).
- The on-disk bare repo path. Find it, do not guess:
  ```sh
  # The repo lives under Forgejo's data dir, in gitea-repositories/<owner>/<repo>.git.
  # Find the data dir from app.ini ([repository].ROOT) or the service env, e.g.
  # /var/lib/forgejo/data or /var/lib/gitea/data. Then:
  REPO="<FORGEJO_DATA>/gitea-repositories/psa-systems/lets-chat.git"
  test -d "$REPO/refs" || { echo "not a bare repo: $REPO"; exit 1; }
  cd "$REPO"
  ```
- A maintenance window is not required, but avoid running `git gc` while a large
  push to this repo is in flight.

## Step 1 - back up the repo first

```sh
ts="$(date +%Y%m%d-%H%M%S)"
tar -czf "/var/tmp/lets-chat.git.pre-lc1008.$ts.tar.gz" -C "$(dirname "$REPO")" "$(basename "$REPO")"
ls -lh "/var/tmp/lets-chat.git.pre-lc1008.$ts.tar.gz"
```

Keep this backup until Step 5 verifies and the team is satisfied, then delete it
(it contains the secrets). Do not copy it anywhere networked.

## Step 2 - list the stale pull refs (dry run, deletes nothing)

A pull ref is **stale** when its tip commit is not reachable from any current
non-pull ref (`refs/heads/*`, `refs/tags/*`). Open same-repo PRs keep their head
branch under `refs/heads/`, so their pull refs are reachable and are left alone;
every frozen pre-rewrite ref is unreachable and is listed here.

```sh
cd "$REPO"

git for-each-ref --format='%(refname)' refs/pull/ > /var/tmp/lc1008-pullrefs.txt
: > /var/tmp/lc1008-stale.txt
while read -r ref; do
  tip="$(git rev-parse "$ref")"
  # Is this tip contained by (reachable from) any live branch or tag? A line is
  # printed only when it is; an empty result means the commit survives solely on
  # this pull ref, i.e. it is a frozen pre-rewrite object -> stale.
  if [ -z "$(git for-each-ref --contains "$tip" --format='x' refs/heads/ refs/tags/ 2>/dev/null)" ]; then
    echo "$ref $tip" >> /var/tmp/lc1008-stale.txt
  fi
done < /var/tmp/lc1008-pullrefs.txt

echo "pull refs total:  $(wc -l < /var/tmp/lc1008-pullrefs.txt)"
echo "stale (to delete): $(wc -l < /var/tmp/lc1008-stale.txt)"
# Spot-check: refs/pull/253/head MUST appear in the stale list.
grep -q '^refs/pull/253/head ' /var/tmp/lc1008-stale.txt \
  && echo "OK: pull/253 flagged stale" || echo "WARN: pull/253 not flagged - stop and investigate"
```

`git for-each-ref --contains <tip> refs/heads/ refs/tags/` prints a line only when
some live branch or tag contains that commit; an empty result means the commit
survives only on the pull ref, i.e. it is a frozen pre-rewrite object. Confirm the
`pull/253` spot-check prints OK before proceeding. If the count looks wrong (near
zero, or all 1088), stop: the criterion or the repo path is off.

## Step 3 - delete the stale refs

```sh
cd "$REPO"
while read -r ref tip; do
  git update-ref -d "$ref" "$tip"
done < /var/tmp/lc1008-stale.txt

# None should remain unreachable-but-present:
echo "pull refs remaining: $(git for-each-ref refs/pull/ | wc -l)"
```

`git update-ref -d "$ref" "$tip"` deletes only if the ref still points where we
read it (a safe compare-and-delete; no force). Any pull refs that remain are the
reachable ones (open PRs) and are intentionally kept.

## Step 4 - drop the now-unreachable objects

```sh
cd "$REPO"
git reflog expire --expire=now --all 2>/dev/null || true   # bare repos usually have none
git gc --prune=now
```

`--prune=now` is required by the ticket AC: without it the objects stay in the
packfiles and remain fetchable by SHA where the server allows it. This is not a
`--force`; it just drops the expiry grace window.

## Step 5 - verify (all four acceptance criteria)

Run the first three **on the host, in `$REPO`**, and the fourth **from a fresh
clone elsewhere** (it proves the server no longer serves it over the wire):

```sh
# AC1: stale closed-PR refs no longer resolve on disk.
git -C "$REPO" rev-parse --verify refs/pull/253/head 2>/dev/null \
  && echo "FAIL: pull/253 still present" || echo "OK: pull/253 gone"

# AC3: the pre-rewrite commit is no longer an object in the repo.
git -C "$REPO" cat-file -e 875b7433 2>/dev/null \
  && echo "FAIL: 875b7433 still present" || echo "OK: 875b7433 pruned"
```

```sh
# AC2 is Step 4 (git gc --prune=now ran).
# AC4: from a throwaway clone, the exposure check returns nothing.
tmp="$(mktemp -d)"; git -C "$tmp" init -q
git -C "$tmp" remote add origin https://dev.a8n.run/psa-systems/lets-chat.git
# Fetching the frozen ref must now fail:
git -C "$tmp" fetch origin 'refs/pull/253/head:refs/heads/check' \
  && echo "FAIL: pull/253/head still fetchable" || echo "OK: pull/253/head refused"
# Fetching the commit by SHA must fail (already refused today, re-confirm):
git -C "$tmp" fetch origin 875b7433 \
  && echo "FAIL: 875b7433 fetchable by SHA" || echo "OK: SHA fetch refused"
rm -rf "$tmp"
```

All four must print `OK`. The AC is met when: the stale refs no longer resolve
(Step 5 AC1), `git gc --prune=now` has run (Step 4 / AC2), the SHA fetch fails
(AC3), and the PR #253 blob read no longer yields the secret (AC4).

## After it is verified

- Delete the Step 1 backup tarball (it still contains the secrets).
- Re-run `forgejo doctor --run check-db-consistency` (or the admin "repository
  health check") so Forgejo's DB view of the repo stays consistent after the
  on-disk ref surgery. The closed PRs keep their records; only their diff view
  is gone, as accepted above.
- Confirm LC-522 AC1 rotation of the exposed `LETS_CHAT_SECRET_KEY` and
  `LETS_CHAT_SMTP_PASSWORD` is done or tracked - that, not this runbook, is what
  ends the exposure for anyone who already cloned.

## Rollback

If anything looks wrong before Step 4's `git gc`, the deleted refs can be
restored from the Step 1 tarball (extract it to a scratch dir and
`git update-ref` each ref back). Once `git gc --prune=now` has run, the objects
are gone and only the tarball can restore them - which is the whole point, so do
not restore unless the deletion was demonstrably wrong.
