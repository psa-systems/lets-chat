# Server/Client Build Split

## Background

The project was scaffolded from `email-client`, which used Dioxus fullstack. As a result, the `desktop` feature currently pulls in `dioxus/server` and `dioxus/fullstack`, bundling the entire Axum server + SQLx + auth stack into the desktop binary. This does not match the intended architecture:

- **Web build** should be the server (Axum + WASM client served over HTTP).
- **Desktop build** should be a pure native client that talks to a remote server.

## Goals

1. Split Cargo features into `server` (web + backend) and `client` (desktop native, no server).
2. Ensure server-only dependencies (sqlx, axum, argon2, tracing-subscriber, etc.) are not compiled into the desktop `client` build.
3. Keep the web build working end-to-end.
4. Keep the desktop build *compiling*, even if the runtime UX for pointing at a remote server is incomplete.

## Non-Goals

- Building a first-launch UI or config-file loader for the desktop server URL. A `LETS_CHAT_SERVER_URL` env var read at startup is sufficient for now.
- Making the desktop build a polished, shippable app — it may be broken at runtime after this change.
- Refactoring server function call sites.

## Design

### Feature restructure (`Cargo.toml`)

**Before:**

```toml
[features]
default = []
server = ["dioxus/server"]
desktop = ["dioxus/desktop", "dioxus/server", "dioxus/fullstack"]
```

**After:**

```toml
[features]
default = []
server = ["dioxus/server"]
client = ["dioxus/desktop"]
```

Dioxus server functions compile to client stubs on non-server builds and dispatch to the configured server URL.

### Dependency gating

Current state: non-WASM backend deps are gated on `cfg(not(target_arch = "wasm32"))`. After the split, the desktop client is also non-WASM but must NOT pull in server deps.

Plan: move server-only deps from the `cfg(not(target_arch = "wasm32"))` target table to an **optional dep list activated by the `server` feature**. Concretely:

- `sqlx`, `axum`, `axum-extra`, `http`, `argon2`, `tracing`, `tracing-subscriber`, `dashmap`, `time` → marked `optional = true`, added to `server = [...]` feature list.
- `tokio` → stays non-WASM (the desktop client also uses async). Keep under `cfg(not(target_arch = "wasm32"))`.
- `rand`, `futures` → keep under `cfg(not(target_arch = "wasm32"))` if the client needs them; otherwise move under `server` feature.

The exact partition will be determined by auditing each dep's usage during implementation.

### `cfg` gate updates

Every `#[cfg(not(target_arch = "wasm32"))]` in `src/` that guards server-only code (DB access, Axum handlers, WebSocket hub, auth logic) will change to `#[cfg(feature = "server")]`. Code that should run on both the server and the desktop client (shared models, HTTP client code) stays gated on `not(target_arch = "wasm32")` or becomes unconditional as appropriate.

Known server-only modules (from CLAUDE.md): `src/db/`, `src/ws/`, server-side portions of `src/server_fns/`, and the Axum bootstrap in `src/main.rs`.

### Entry point (`src/main.rs`)

`main.rs` currently routes between desktop/web/WASM based on platform. After the split:

- **Web build** (`--features server`): unchanged — starts Axum, serves WASM client.
- **Desktop build** (`--features client`): launches the Dioxus desktop runtime only. At startup, reads `LETS_CHAT_SERVER_URL` and calls the Dioxus API to set the server URL used by client-side server function stubs. (Exact API call confirmed during implementation; likely `dioxus::fullstack::set_server_url` or equivalent.)
- **WASM target**: unchanged.

### `justfile`

- `dev-desktop` → `dx serve --platform desktop --features client` (or whatever `dx` requires to pick up the renamed feature).
- `dev-web-local`, `dev-web`, `build`, etc. → unchanged (still use `server` feature via `dx --platform web`).

### Docker

`ci-build/Dockerfile.web` and `ci-build/Dockerfile.desktop-linux` will be reviewed; if they pass explicit feature flags they need updating for the rename.

## Testing & Verification

1. `just check` (server-side cargo check) compiles.
2. `just check-web` (WASM target) compiles.
3. `just dev-web-local` starts and serves HTTP 200 (same bar as `just verify`).
4. `cargo build --features client --no-default-features` compiles a desktop client binary — no sqlx/axum in the dep graph (spot-check via `cargo tree`).
5. Desktop runtime is allowed to be broken; no e2e desktop test required.

## Risks

- **Dep gating errors**: a missed `cfg` gate means the client build pulls in sqlx or fails to compile. Mitigation: `cargo tree --features client --no-default-features` to confirm no server deps leak in.
- **Server function URL wiring**: the Dioxus API for setting the remote server URL on desktop may need a small dive into Dioxus 0.7 docs. If the API has changed or isn't stable, fall back to documenting the limitation — the feature split still lands, desktop UX follows up.
- **`dx` feature-flag conventions**: `dx serve --platform desktop` may assume the feature is named `desktop`. If renaming breaks `dx`, two fallbacks: (a) keep the feature name `desktop` but change its contents, (b) pass `--features client` explicitly.
