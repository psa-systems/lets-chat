# Operating a deployment

Health probes and the boot dependencies an orchestrator needs to know about.
Environment variables are in [configuration.md](configuration.md); release and
upgrade conventions are in [releasing.md](releasing.md).

## Operational probes

Three unauthenticated diagnostic endpoints, all exempt from the maintenance-mode gate so they answer during a window:

| Endpoint | Purpose | Checks |
|---|---|---|
| `GET /healthz` | Liveness. Answers as soon as the process is listening. This is what the container `HEALTHCHECK` targets and what an orchestrator liveness gate should use. | Nothing (no DB, no SSO) - so it never false-negatives on a dependency hiccup and never masks a wedged process. |
| `GET /readyz` | Readiness. `200` when the instance can serve, `503` when a backing store is down. Use it for load-balancer draining and monitoring, never as the container healthcheck (a transient dependency blip would restart an otherwise-fine process). | Pings the auth, chat, and settings SQLite pools; reports SSO-client presence (a runtime OP outage degrades login only and does not flip the `503`). |
| `GET /version` | Build metadata (version, git hash, build date) so operators can confirm exactly what is running. | Nothing. |

## SSO troubleshooting

`/login?sso_error=<code>` is how a failed Bunyip SSO attempt reports why. Codes and what to do about each:

| Code | Cause | Action |
|---|---|---|
| `dance` | The OIDC callback could not be completed (state/nonce mismatch, or the resolved identity collided with a bot row). | Have the user retry. Recurring failures point at a clock skew or proxy issue between this app and Bunyip. |
| `op` | Bunyip (the OP) rejected the sign-in attempt. | Check Bunyip's own logs for the rejection reason. Not a deployment fault in this app. |
| `banned` | The resolved user account is suspended. | Expected; the account is banned. Not a bug. |
| `identity_conflict` | The verified email from Bunyip is already linked to a different account. | An admin unlinks the stale row (`POST /admin/users/{id}/unlink-sso`) before the user can sign in with this identity. |
| `internal` | An unexpected error (database or app) during session creation or resolution. | Check server logs around the timestamp of the report for the underlying error. |
| `unconfigured` | This server was booted with `LETS_CHAT_DEV_NO_SSO` and has no SSO client at all. | Development-only. Not expected in a real deployment. |
| `approval` | The user's emailed approval code expired, was used, or hit the attempt cap. | Have them sign in again; a new code is emailed. Not a deployment fault. |

## Boot dependencies

The Bunyip SSO OP is a hard boot dependency: `main` exits (non-zero) if discovery or JWKS is unreachable at startup, and with `restart: unless-stopped` that crash-loops the container, which presents to a reverse proxy as a gateway timeout. `/healthz` is deliberately dependency-free so that a container which HAS booted is never culled for an unrelated dependency, and so a reverse proxy always has a real upstream to route to.

The runtime image runs as the unprivileged `appuser`, exposes port 8080, and keeps every SQLite database under `LETS_CHAT_DATA_DIR` (`/data`), which is the only path that needs a volume.
