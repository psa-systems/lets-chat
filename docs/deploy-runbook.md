# Let's Chat: Deploy Runbook (Production + Staging)

## Topology

| Env | Host | Compose dir | Public URL | Image tag policy |
|-----|------|-------------|------------|------------------|
| Production | nc-01 | `~/docker/server/nc-01/lets-chat-psa` | https://chat.psa.systems | Pinned (`:vX.Y.Z`) |
| Staging | c-01 | `~/docker/server/c-01/lets-chat-psa` | https://chat.a8n.systems | Tracks `:latest` |

- Image registry (post psa-systems move): `dev.a8n.run/psa-systems-private/lets-chat`
- Old path `dev.a8n.run/a8n-tools-private/lets-chat` holds ONLY pre-move images (<= v0.1.0). Post-move tags (`:latest`, `>= v0.2.0`) live only at the new path. A stale pin to the old org = `manifest unknown`.
- Image pin lives in each host's `compose-variables.yml`, not the compose file.
- `just app-restart` / `just app-start` run `docker compose` against the LOCAL daemon. Run them ON the target server. No remote-deploy tooling; each server is driven by hand.
- Same repo pattern applies to c-01's legacy `lets-chat` dir if still present; only `lets-chat-psa` is live.

## 0. Preflight (local, before cutting anything)

```nushell
cd ~/lets-chat
git fetch origin
git switch main
git pull --ff-only
just check          # fmt + clippy + tests must be green
```

Confirm CI is green on `main` in Forgejo (`build-oci-image.yml` and `check.yml`).

Note: `build-oci-image.yml` triggers ONLY on `server/src/**`, migrations, templates, assets, `package.json`, `bun.lock`, `tailwind.config.js`, `Cargo.toml`, `Cargo.lock`, `ci-build/Dockerfile.web`, `get-tags.nu`, and the workflow itself. `tests/**` does NOT trigger a build.

## 1. Cut the release (produces the pinned tag)

```nushell
cd ~/lets-chat
just create-release minor      # or: major | hotfix
```

This bumps `Cargo.toml`, pushes `release/vX.Y.Z`, opens the PR. On merge, CI tags the commit and `build-oci-image.yml` publishes BOTH `:vX.Y.Z` and `:latest` to `dev.a8n.run/psa-systems-private/lets-chat`.

Wait for the image job to finish green before touching any server. Verify the tag exists:

```nushell
# on any host with registry creds
docker pull dev.a8n.run/psa-systems-private/lets-chat:vX.Y.Z
```

## 2. Deploy STAGING first (c-01)

Staging tracks `:latest`, so a merge that rebuilt `:latest` is picked up by a restart alone. No pin edit needed unless the pin drifted.

```nushell
# on c-01
cd ~/docker/server/c-01/lets-chat-psa
git pull --ff-only            # pick up any compose/pin changes
docker compose pull           # or: just app-pull, if defined
just app-restart
```

Verify:

```nushell
http get https://chat.a8n.systems/version
# expect git_version == vX.Y.Z (or the latest main hash)
```

Smoke test on staging:
- Load https://chat.a8n.systems, sign in with Bunyip SSO (round trip completes, lands in chat).
- Send a message, confirm it renders.
- Check presence/roster updates.

## 3. Bump the PRODUCTION pin (nc-01 is pinned, not :latest)

Prod does NOT track `:latest`. Edit the pin in the docker repo, PR it, merge, THEN pull on nc-01.

```nushell
cd ~/docker
git switch main
git pull --ff-only
git switch --create chore/lets-chat-vX.Y.Z
# edit ~/docker/server/nc-01/lets-chat-psa/compose-variables.yml
#   image tag: psa-systems-private/lets-chat:vX.Y.Z
#   confirm registry org is psa-systems-private (NOT a8n-tools-private)
git commit --all --message "chore(lets-chat): pin prod to vX.Y.Z"
git push --set-upstream origin chore/lets-chat-vX.Y.Z
# open + merge the PR (fj pr create on dev.a8n.run)
```

## 4. Deploy PRODUCTION (nc-01)

```nushell
# on nc-01
cd ~/docker/server/nc-01/lets-chat-psa
git pull --ff-only            # pick up the new pin
docker compose pull           # pull vX.Y.Z BEFORE restart (avoids downtime gap)
just app-restart
```

WARNING: `just app-restart` removes the old container. If the new image is not present (bad pin / not pulled), prod goes down until the pin is fixed. Always `docker compose pull` and confirm success before `app-restart`.

Verify:

```nushell
http get https://chat.psa.systems/version
# expect git_version == vX.Y.Z
```

Prod smoke test:
- Sign in with Bunyip SSO.
- Send a message; confirm delivery + render.
- Confirm presence/roster.

## 5. Post-deploy

- Watch logs briefly on each host:
  ```nushell
  docker compose logs --follow --tail 100
  ```
- Update the tracking ticket (e.g. LC-585 / LC-620) with env + version + result.
- Prod `:latest` is never used; only staging follows it.

## Rollback

Prod (pinned): repoint `compose-variables.yml` to the previous good `:vX.Y.(Z-1)`, merge, `git pull --ff-only`, `docker compose pull`, `just app-restart`.

Staging (`:latest`): revert or re-tag `:latest` upstream, or temporarily pin c-01's `compose-variables.yml` to a known-good `:vX.Y.Z` and restart.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `manifest unknown` on pull | Pin points at old `a8n-tools-private` org, or tag not built yet | Repoint to `psa-systems-private`; confirm CI image job green |
| Version endpoint still shows old build after restart | Host repo not pulled, or `:latest` not re-pulled | `git pull --ff-only` then `docker compose pull` before restart |
| SSO `sso_error=internal` | App-level defect, NOT version drift | Check logs: `docker compose logs --tail 200 | find "resolve_or_provision_user"` |
| Prod down after restart | New image absent when old container removed | `docker compose pull` succeeded? Fix pin, pull, restart |

### Log grep (Nushell)

```nushell
docker compose logs --since 5m | find "bunyip_sso"
# combined stdout+stderr in Nushell: use  o+e>|  not  2>&1
# docker --since wants Go duration:  5m  not  5min
```
