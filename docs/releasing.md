# Releasing and operator-visible changes

How lets-chat cuts releases, and the convention for recording changes that affect operators. Read this before tagging a release or before opening a PR that changes operator-visible behavior.

## The release process (today)

The project ships two ways:

- **Server**: the `latest` OCI image, built off `main` (`build-oci-image.yml`). Operators run it directly. There is **no per-commit version**; `main` is the release.
- **Desktop**: a self-updating Tauri 2 app. `publish-release.yml` fires on a `v*` tag push, cross-builds the Linux/Windows binaries, uploads them plus a `latest.json` manifest to Forgejo Generic Packages (the hand-download path), and pushes each binary to the container registry as an OCI artifact (the path the app's self-updater pulls). The only credential the release needs is the packages PAT, used for both.

### Desktop distribution is membership-gated (LC-733)

The desktop binaries are **not public**. They live under a private org, so an anonymous download answers 401, and the updater has to prove the user's membership to fetch one.

- **The release publishes both paths.** The Generic Packages upload is unchanged and remains the hand-download path. On top of it, the release pushes each binary to the container registry (`{owner}/{package}`) with `oras`, as an OCI artifact whose manifest carries one layer (the binary) and the release version in its `org.opencontainers.image.version` annotation. Each binary lands under two tags: `latest-{os}-x86_64`, which is what the shipped client resolves, and `{version}-{os}-x86_64`, the rollback target. LC-831: this half was missing, so every update check resolved a tag that had never existed.
- **The client pulls over the OCI distribution API.** The updater fetches `{registry}/v2/{repository}/manifests/{tag}` for its platform (`latest-linux-x86_64` / `latest-windows-x86_64`), reads the release version from the manifest's `org.opencontainers.image.version` annotation and the single artifact layer's SHA-256 digest, then downloads that one blob. It is an artifact pull, not an image pull: no layer stack, no extraction. `latest.json` is still published but the updater no longer reads it.
- **The release build compiles in where it publishes to.** `publish-release.yml` derives the registry and repository from the same `PACKAGE_OWNER` / `PACKAGE_NAME` it pushes to and passes them as `LETS_CHAT_UPDATE_REGISTRY_URL` / `LETS_CHAT_UPDATE_REPOSITORY` build args, so a shipped binary cannot poll somewhere the release never writes. The literals in `desktop/src/update.rs` are only the fallback for a build with no injection. `ci-build/check-update-injection.nu` (a `just check` recipe and a check.yml step) fails the build if a name the client reads is not injected, if an injected name is read by nothing, or if a platform tag the client resolves is not pushed.
- **The registry is Bunyip** once its OCI proxy is published: it proxies Forgejo and is what accepts a Let's Chat user's token. The client speaks plain OCI to it; a divergence from the distribution spec is a Bunyip-side fix rather than a client special case. That endpoint does not exist yet (LC-733's merge gate), so the release currently injects the Forgejo host that serves the packages and the registry today (`https://dev.a8n.run`); pointing at Bunyip is then a one-value change in the workflow, not a client change.
- **The credential is the user's own Bunyip login.** `GET /desktop/registry-token` returns the access token from that user's Bunyip sign-in to their authenticated session (401 when signed out); the desktop bridge forwards it to the native side and the updater stores it in its config file. There is no second sign-in and no pasted token. A user without entitlement gets a spelled-out authorization error from the updater, not a generic network failure.
- **The bearer never crosses an origin.** Registries redirect blob GETs to storage backends; the updater drops the `Authorization` header on any cross-origin hop and keeps the per-hop public-IP filter (LC-210) unchanged.
- **Integrity** is still the artifact SHA-256, now the layer digest from the manifest, checked before the in-place replace. It catches a corrupt or truncated download, not an attacker who controls the source; the authenticated fetch is what makes the source trustworthy (LC-709).

Operator overrides, all optional: `LETS_CHAT_UPDATE_REGISTRY_URL` (registry root), `LETS_CHAT_UPDATE_REPOSITORY` (`{owner}/{package}`), `LETS_CHAT_UPDATE_TAG`, `LETS_CHAT_UPDATE_TOKEN` (a credential for a headless check), and `LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE=1` to exempt only the initial URL from the public-IP filter for an internal mirror. The first two are also what the release build injects, under the same names, and the run-time value wins over the compiled one.

A missing artifact is now reported rather than retried in silence: a 404 on the manifest means this binary polls a coordinate that holds nothing for its platform, which no amount of waiting fixes, so the GUI raises it the same way it raises an entitlement refusal.

Tagging is automated and **semver, branch-driven**:

1. `just create-release <major|minor|hotfix>` (from `common/common.just`, LC-761) bumps the `[workspace.package]` version, syncs `Cargo.lock`, commits `Release vX.Y.Z` on a `release/vX.Y.Z` branch, and opens a PR via `fj`.
2. On merge, `.forgejo/workflows/create-release.yml` calls the reusable `psa-systems/common/.forgejo/workflows/create-release.yml`, which tags the merge commit and creates a Forgejo Release. The body is **one line per pull request merged to main since the previous tag** (`git log --oneline --first-parent`, with each `Merge pull request '<title>' (#N)` rewritten to `<title> (#N)`). This replaced the former private copy whose plain `git log --oneline` double-listed every PR as both its merge commit and the branch-side commits it already contained (the v0.3.0 eight-lines-for-four-PRs bug).
3. The `v*` tag triggers `publish-release.yml` (desktop artifacts).

Cutting a tag is the only thing that creates the `latest-{os}-x86_64` artifacts: no push to `main` and no manual step publishes them, so the desktop update path is exactly as current as the last release run.

## The decision: hybrid (CHANGELOG.md, curated at release time)

LC-209 weighed three options against the release reality above:

- **(a) Per-PR CHANGELOG.md.** Rejected as the *sole* mechanism. With ~1000 commits in the first weeks, no CI gate, and high velocity, a "every PR updates CHANGELOG.md" rule is the discipline that won't hold; a stale changelog is worse than none.
- **(b) No CHANGELOG.md; PR descriptions + git log are the changelog.** This is effectively the status quo (the auto-generated release body *is* `git log --oneline`). Its weakness: an operator landing on the repo sees no changelog, and a raw commit-subject firehose does not surface "default-on change, set this env var" or "security fix, upgrade promptly" with the action text.
- **(hybrid) CHANGELOG.md curated at release time, written manually by the release-cutter.** Chosen.

Why the hybrid works *here* specifically (the "won't hold" risk, answered):

- There **is** a release trigger to anchor to (`just create-release`); the release PR body already has a tempfile seam "so the changelog can grow later." The infrastructure was built to carry curated release content.
- **Per-PR friction is zero.** PRs do not touch `CHANGELOG.md`. Only the release-cutter does, once per release.
- `CHANGELOG.md` **cannot go stale.** It records tagged releases only (append-at-release). There is no half-updated "Unreleased" body rotting between releases.

The Forgejo release body keeps its existing auto `git log` (the zero-maintenance firehose); `CHANGELOG.md` is the curated operator-facing view in the repo root.

## What counts as "operator-visible"

A change is operator-visible (and belongs in `CHANGELOG.md` at the next release) if, on upgrade, an operator might have to **do something or know something**:

- A new or changed **environment variable**, especially a **default** (default-on behavior, default-off destructive feature).
- A **config-format** or schema change that needs migration or re-entry.
- A **security fix** present in shipped versions (upgrade-promptly, with the vector).
- A **behavior change** visible on upgrade (new outbound traffic, changed endpoint/response, changed retention/deletion behavior).
- A **deprecation or removal** of a flag, endpoint, or feature.
- A change to **integration contracts** other software depends on (webhook payloads/events, API shapes).

Not operator-visible (git history is the record): internal refactors, test hygiene, performance work with no behavior change, decoder/dependency hardening with no operator action, doc-only changes.

## Responsibilities

- **PR author**: no added per-PR step. `CHANGELOG.md` is not touched on the PR path; a clear PR description and commit subject are what the release-cutter reads from.
- **Release-cutter** (whoever runs `just create-release`): before merging the release PR, reads the PR titles and descriptions merged since the last tag (`git log --oneline --first-parent <last-tag>..HEAD` is the same range the release body uses), picks out the operator-visible ones against the list above, and writes them into `CHANGELOG.md` under the new version - Security first, then Changed / Added / Fixed / Deprecated - editing the release PR body (it already takes a tempfile) to mirror them. Internal commits are left to the auto `git log` body.

## Cutting a release (checklist)

1. Skim the PR titles and descriptions merged since the last tag (`git log --oneline --first-parent <last-tag>..HEAD`) and note the operator-visible ones against the list above. For the **first** tag, the range is the whole history; use the `## Pre-release` seed already in `CHANGELOG.md` as the starting point.
2. `just create-release <major|minor|hotfix>`.
3. On the `release/vX.Y.Z` branch, write the noted items into a new `## [vX.Y.Z] - <date>` section (Security / Changed / Added / Fixed / Deprecated), and enrich the release PR body tempfile to match.
4. Merge the release PR. `create-release.yml` tags + publishes; `publish-release.yml` ships the desktop artifacts.
