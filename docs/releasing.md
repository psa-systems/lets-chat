# Releasing and operator-visible changes

How lets-chat cuts releases, and the convention for recording changes that affect operators. Read this before tagging a release or before opening a PR that changes operator-visible behavior.

## The release process (today)

The project ships two ways:

- **Server**: the `latest` OCI image, built off `main` (`build-oci-image.yml`). Operators run it directly. There is **no per-commit version**; `main` is the release.
- **Desktop**: a self-updating Tao+Wry app. `publish-release.yml` fires on a `v*` tag push, cross-builds the Linux/Windows binaries, and uploads them plus a `latest.json` manifest to Forgejo Generic Packages. The app's updater reads `latest.json` to discover the newest version. The manifest is Ed25519-signed (`latest.json.sig`) and each binary's SHA-256 is verified before the in-place replace; provisioning the signing key (`DESKTOP_UPDATE_SIGNING_KEY` secret + `DESKTOP_UPDATE_PUBLIC_KEY` variable) is a one-time step in `docs/desktop-update-signing.md`. Until that key is provisioned the updater fails closed (refuses to self-update), and the publish workflow refuses to ship an unsigned manifest.

Tagging is automated and **semver, branch-driven**:

1. `just create-release <major|minor|hotfix>` bumps the workspace version, commits `Release vX.Y.Z` on a `release/vX.Y.Z` branch, and opens a PR via `fj`.
2. On merge, `create-release.yml` tags the merge commit and creates a Forgejo Release whose body is an **auto-generated `git log --oneline` between the previous tag and HEAD**.
3. The `v*` tag triggers `publish-release.yml` (desktop artifacts).

As of this writing **no tag has been cut** (the repo is at `v0.1.0`, the seed version). The machinery is built and dormant.

## The decision: hybrid (CHANGELOG.md, curated at release time)

LC-209 weighed three options against the release reality above:

- **(a) Per-PR CHANGELOG.md.** Rejected as the *sole* mechanism. With ~1000 commits in the first weeks, no CI gate, and high velocity, a "every PR updates CHANGELOG.md" rule is the discipline that won't hold; a stale changelog is worse than none.
- **(b) No CHANGELOG.md; PR descriptions + git log are the changelog.** This is effectively the status quo (the auto-generated release body *is* `git log --oneline`). Its weakness: an operator landing on the repo sees no changelog, and a raw commit-subject firehose does not surface "default-on change, set this env var" or "security fix, upgrade promptly" with the action text.
- **(hybrid) CHANGELOG.md curated at release time, fed by a grep-able marker.** Chosen.

Why the hybrid is sustainable *here* specifically (the "won't hold" risk, answered):

- There **is** a release trigger to anchor to (`just create-release`); the release PR body already has a tempfile seam "so the changelog can grow later." The infrastructure was built to carry curated release content.
- **Per-PR friction is zero.** PRs do not touch `CHANGELOG.md`. Only the release-cutter does, once per release.
- The update is **mechanical, not from memory.** The cutter greps the operator-action markers since the last tag and folds them in. They never have to remember what changed.
- `CHANGELOG.md` **cannot go stale.** It records tagged releases only (append-at-release). The between-release delta lives in immutable git markers and is always reconstructable via `git log --grep`. There is no half-updated "Unreleased" body rotting between releases.

The Forgejo release body keeps its existing auto `git log` (the zero-maintenance firehose); `CHANGELOG.md` is the curated operator-facing view in the repo root.

## What counts as "operator-visible"

A change is operator-visible (and needs the marker below) if, on upgrade, an operator might have to **do something or know something**:

- A new or changed **environment variable**, especially a **default** (default-on behavior, default-off destructive feature).
- A **config-format** or schema change that needs migration or re-entry.
- A **security fix** present in shipped versions (upgrade-promptly, with the vector).
- A **behavior change** visible on upgrade (new outbound traffic, changed endpoint/response, changed retention/deletion behavior).
- A **deprecation or removal** of a flag, endpoint, or feature.
- A change to **integration contracts** other software depends on (webhook payloads/events, API shapes).

Not operator-visible (omit the marker; git history is the record): internal refactors, test hygiene, performance work with no behavior change, decoder/dependency hardening with no operator action, doc-only changes.

## The marker convention

Operator-visible PRs and their merge commits carry a marker so they are greppable and surface in the auto-generated release notes.

- **Required - subject flag.** Put `[operator-action]` in the **PR title** and the **commit subject**. Because the Forgejo release body is `git log --oneline`, the flag shows up inline in the release notes, and it is greppable:

  ```
  git log --grep='\[operator-action\]' <last-tag>..HEAD
  ```

  Example subject: `feat(bridges): default-on foreign avatar proxy [operator-action] (LC-78)`

- **Recommended - body trailer.** Add an `Operator-Action:` trailer to the commit body (and a matching line in the PR description) carrying the **one-line instruction** - the thing the operator must do or know. The subject flag flags the commit; the trailer captures the action text at the source so the release-cutter does not have to reverse-engineer it.

  ```
  Operator-Action: Bridge avatar fetching is now ON by default; set
  LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false to keep the old reject behavior.
  ```

  Grep the trailers with `git log --grep='^Operator-Action:' <last-tag>..HEAD`.

For a **security** change present in shipped versions, also say so in the trailer ("upgrade promptly", the vector) so it lands in the changelog's Security section.

## Responsibilities

- **PR author**: decides if the change is operator-visible (list above), adds `[operator-action]` to the title + commit subject, and writes the `Operator-Action:` trailer. This is the only added per-PR step, and only for the rare operator-visible PR.
- **Release-cutter** (whoever runs `just create-release`): before merging the release PR, runs the greps above for the range since the last tag, and folds the results into `CHANGELOG.md` under the new version - Security first, then Changed / Added / Fixed / Deprecated - editing the release PR body (it already takes a tempfile) to mirror them. Internal commits are left to the auto `git log` body.

## Cutting a release (checklist)

1. `git log --grep='\[operator-action\]' <last-tag>..HEAD` (and the `Operator-Action:` trailer grep) to gather the operator-visible delta. For the **first** tag, the range is the whole history; use the `## Pre-release` seed already in `CHANGELOG.md` as the starting point.
2. `just create-release <major|minor|hotfix>`.
3. On the `release/vX.Y.Z` branch, move the gathered items from `CHANGELOG.md`'s notes into a new `## [vX.Y.Z] - <date>` section (Security / Changed / Added / Fixed / Deprecated), and enrich the release PR body tempfile to match.
4. Merge the release PR. `create-release.yml` tags + publishes; `publish-release.yml` ships the desktop artifacts.

## Future option (not in scope for LC-209)

`create-release.yml` could be enhanced to grep the `[operator-action]` / `Operator-Action:` markers itself and prepend an "Operator actions" section to the release body automatically, reducing the manual step in (3). It is deliberately not done here: it modifies dormant CI that has never run and cannot be exercised until a first real release. Revisit when the first tag is cut.
