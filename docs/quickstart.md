# Quickstart

Get a Let's Chat server running, either as a container or as a local dev stack.
Every environment variable named here is described in
[configuration.md](configuration.md).

## Docker (recommended)

```nu
docker build --tag lets-chat --file ci-build/Dockerfile.web .
docker run --publish 8080:8080 --volume lets-chat-data:/data \
  --env LETS_CHAT_BUNYIP_SSO_ISSUER=https://your-op.example.com \
  --env LETS_CHAT_BUNYIP_SSO_CLIENT_ID=... \
  --env LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET=... \
  --env LETS_CHAT_BUNYIP_SSO_REDIRECT_URI=https://chat.example.com/auth/bunyip/callback \
  lets-chat
```

Then open `http://localhost:8080` and click "Sign in with Bunyip". The first user to sign in is automatically promoted to Admin.

> Authentication is Bunyip SSO only (LC-22): the four `LETS_CHAT_BUNYIP_SSO_*` vars are **mandatory** and the server refuses to start without them. The OIDC client must be registered on your Bunyip OP with the `redirect_uri` above in its `redirect_uris`. There is no local-auth fallback.

> **Upgrading?** Read [`CHANGELOG.md`](../CHANGELOG.md) first. It calls out default-on behavior changes, security fixes, and env vars you may need to set before upgrading. The convention behind it is in [releasing.md](releasing.md).

## Local development

The host needs only Docker, [just](https://github.com/casey/just), and (optionally) [Nushell](https://www.nushell.sh/) for the `verify` recipe. Cargo and Bun run inside containers via the wrappers in `dev/`.

The root `justfile` imports the shared recipes from the `common` submodule, so after a fresh clone run `git submodule update --init` (or clone with `--recurse-submodules`); without it every `just` command is a parse error.

```nu
just dev-web-local
```

Then open `http://localhost:18080`.

`just dev-web-local-mock` layers a stub OP (`dev/mock-oidc.py`) on top so a sign-in actually completes without the bunyip dev-sso stack. Both recipes set `SETUP_DEFAULT_ADMIN` (DEV-300), which seeds an admin account for `dev@example.test` while no admin exists; the stub's primary identity uses that address and claims `bunyip_role: admin`, so the first sign-in lands on it. Set a `mock_user=<name>` cookie on the stub to sign in as an ordinary extra user instead.

The stub's issuer is the compose-internal host `mock-oidc:9000`, which the browser on your host cannot resolve on its own. Start the browser with `--host-resolver-rules="MAP mock-oidc 127.0.0.1"`, or add the equivalent mapping, so one issuer string stays valid for both the container and the browser.

To build and run the production-shape image instead:

```nu
just run
```

That builds `ci-build/Dockerfile.web` via `compose.yml` and serves on `http://127.0.0.1:8080`. Supply the mandatory Bunyip SSO vars (and any optional features) with an env file: copy `.env.standalone`, fill it in, and either add `env_file: [.env.standalone]` to `compose.yml` or pass `--env-file .env.standalone`.

Run `just --list` to see all available recipes.

> **Huddles or stage audio do not start locally?** `dev-web-local`, `dev-web-local-mock`, `dev-web-local-saas` and `dev-web-local-saas-mock` all depend on `vendor-js`, which vendors the LiveKit browser SDK to `server/assets/vendor/livekit-client.umd.min.js` before the server starts. If you bypassed the recipes (e.g. running the binary directly against a bind-mounted `server/`), run `just vendor-js` yourself first.

## Local smoke test

`just verify` builds the release binary, boots it in a container on port 18080 and checks that the login page renders. Sign-in is Bunyip SSO only and the server refuses to start without it, so the smoke sets `LETS_CHAT_DEV_NO_SSO=1`: a development-only opt-out that boots with no identity provider at all (nobody can sign in; the SSO routes answer with a "not configured" login error). Never set it on a real deployment. `./dev/server-up [--release]` runs the binary you last built with `./dev/cargo build` (same image, same target volume, so nothing recompiles) and passes that variable and the `LETS_CHAT_BUNYIP_SSO_*` variables through from your shell if set.
