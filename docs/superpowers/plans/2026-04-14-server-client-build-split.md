# Server/Client Build Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `server` / `desktop` Cargo feature split with `server` / `client`, where `client` is a pure Dioxus desktop build with no Axum/SQLx server code compiled in.

**Architecture:** Rename `desktop` → `client`, drop `dioxus/server` and `dioxus/fullstack` from it, mark all server-only deps as `optional = true` and activate them via the `server` feature. Flip every `#[cfg(not(target_arch = "wasm32"))]` that guards server-only code to `#[cfg(feature = "server")]`. Remove the embedded-server branch from `main.rs` on desktop. The web build stays functional end-to-end; the desktop build must compile but may be runtime-broken.

**Tech Stack:** Rust, Dioxus 0.7, Axum 0.8, SQLx, `dx` CLI.

---

## File Structure

Files that will be touched:

- **Modify** `Cargo.toml` — feature list and dep optional flags.
- **Modify** `src/main.rs` — replace `feature = "desktop"` branch with a `feature = "client"` branch that only launches Dioxus desktop; remove embedded-server startup.
- **Modify** `src/lib.rs` — gate `db` / `ws` on `feature = "server"`.
- **Modify** `src/db/mod.rs` — change 19 `cfg(not(target_arch = "wasm32"))` gates to `cfg(feature = "server")`.
- **Modify** `src/ws/mod.rs` — gate `handler` / `hub` on `feature = "server"`.
- **Modify** `src/models/user.rs` — gate `UserRecord` on `feature = "server"`.
- **Modify** `src/models/session.rs` — same treatment.
- **Modify** `src/server_fns/chat.rs` — review gate at line 5.
- **Modify** `justfile` — `dev-desktop` and any feature-flag-aware recipes.
- **Modify** `ci-build/Dockerfile.desktop-linux` — feature rename if it references `--features desktop`.
- **Modify** `ci-build/Dockerfile.web` — sanity-check feature flags.

Files that should NOT change:
- `src/components/use_websocket.rs` — client-side WebSocket gate stays `cfg(not(target_arch = "wasm32"))` (runs on native desktop too).
- `src/server_fns/auth.rs:188` — this is the non-WASM fallback for `clear_session_cookie`, applies to both server and desktop client. Leave as-is.
- `src/routes.rs`, `src/components/**` — should compile unchanged once feature gates are correct.

---

### Task 1: Audit and rename `desktop` feature to `client` in `Cargo.toml`

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Open `Cargo.toml` and replace the `[features]` section**

Replace:

```toml
[features]
default = []
server = ["dioxus/server"]
desktop = ["dioxus/desktop", "dioxus/server", "dioxus/fullstack"]
```

With:

```toml
[features]
default = []
server = [
    "dioxus/server",
    "dep:sqlx",
    "dep:axum",
    "dep:axum-extra",
    "dep:http",
    "dep:argon2",
    "dep:tracing",
    "dep:tracing-subscriber",
    "dep:dashmap",
    "dep:time",
]
client = ["dioxus/desktop"]
```

- [ ] **Step 2: Move server-only deps into a single dep table and mark them optional**

Replace the whole `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` block:

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
axum = { version = "0.8", features = ["ws"] }
http = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
argon2 = "0.5"
rand = "0.8"
axum-extra = { version = "0.10", features = ["cookie"] }
time = "0.3"
dashmap = "6"
futures = "0.3"
```

With two blocks — one for deps the client also needs (stays cfg-gated on non-WASM), one for server-only (unconditional but optional, activated via the `server` feature):

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["full"] }
rand = "0.8"
futures = "0.3"

[dependencies.sqlx]
version = "0.8"
features = ["runtime-tokio", "sqlite"]
optional = true

[dependencies.axum]
version = "0.8"
features = ["ws"]
optional = true

[dependencies.axum-extra]
version = "0.10"
features = ["cookie"]
optional = true

[dependencies.http]
version = "1"
optional = true

[dependencies.argon2]
version = "0.5"
optional = true

[dependencies.tracing]
version = "0.1"
optional = true

[dependencies.tracing-subscriber]
version = "0.3"
features = ["env-filter"]
optional = true

[dependencies.dashmap]
version = "6"
optional = true

[dependencies.time]
version = "0.3"
optional = true
```

- [ ] **Step 3: Verify the web build still compiles**

Run: `cargo check --features server --no-default-features`
Expected: compiles clean (warnings about unused cfg are OK).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: rename desktop feature to client and gate server deps"
```

---

### Task 2: Flip cfg gates from `not(target_arch = "wasm32")` to `feature = "server"`

Server-only modules must be compiled only when the `server` feature is on. On a `client` build (desktop, non-WASM) these must NOT compile, because they depend on sqlx/axum/argon2 which are only present with `server`.

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/db/mod.rs`
- Modify: `src/ws/mod.rs`
- Modify: `src/models/user.rs`
- Modify: `src/models/session.rs`
- Modify: `src/server_fns/chat.rs`

- [ ] **Step 1: Update `src/lib.rs`**

Replace:

```rust
#[cfg(not(target_arch = "wasm32"))]
pub mod db;
pub mod models;
pub mod server_fns;
pub mod ws;
```

With:

```rust
#[cfg(feature = "server")]
pub mod db;
pub mod models;
pub mod server_fns;
pub mod ws;
```

- [ ] **Step 2: Update `src/ws/mod.rs`**

Replace:

```rust
pub mod events;
#[cfg(not(target_arch = "wasm32"))]
pub mod handler;
#[cfg(not(target_arch = "wasm32"))]
pub mod hub;
```

With:

```rust
pub mod events;
#[cfg(feature = "server")]
pub mod handler;
#[cfg(feature = "server")]
pub mod hub;
```

- [ ] **Step 3: Update `src/db/mod.rs` — replace every `cfg(not(target_arch = "wasm32"))` with `cfg(feature = "server")`**

Run this sed-equivalent via the Edit tool with `replace_all`:
- `old_string`: `#[cfg(not(target_arch = "wasm32"))]`
- `new_string`: `#[cfg(feature = "server")]`

Confirm with: `grep -n 'cfg' src/db/mod.rs` — all gates should now say `feature = "server"`.

- [ ] **Step 4: Update `src/models/user.rs`**

Change the single `#[cfg(not(target_arch = "wasm32"))]` guarding `UserRecord` to:

```rust
#[cfg(feature = "server")]
```

- [ ] **Step 5: Update `src/models/session.rs`**

Replace the single gate on line 2:
- `#[cfg(not(target_arch = "wasm32"))]` → `#[cfg(feature = "server")]`

- [ ] **Step 6: Update `src/server_fns/chat.rs` line 5**

Replace:
- `#[cfg(not(target_arch = "wasm32"))]` → `#[cfg(feature = "server")]`

(This is a server-only imports/helpers block. Verify by reading the surrounding context first — if it's a `use` for sqlx/db, the change is correct.)

- [ ] **Step 7: Verify the web build compiles**

Run: `cargo check --features server --no-default-features`
Expected: compiles. If any `db::` or `ws::handler` reference fails to resolve outside `feature = "server"`, that call site is itself server-only and needs a gate added in Task 3.

- [ ] **Step 8: Commit**

```bash
git add src/
git commit -m "refactor: gate server-only modules on feature = server"
```

---

### Task 3: Rewrite `src/main.rs` desktop branch as a pure client launcher

Today `main.rs` unconditionally references `db::set_data_dir`, `tracing_subscriber`, and `build_server_router` — all of which only exist with `feature = "server"`. On a `client` build these references must not be compiled.

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the top-level cfg gates to be feature-based, not arch-based**

Replace lines 1–12:

```rust
use dioxus::prelude::*;

mod components;
#[cfg(not(target_arch = "wasm32"))]
mod db;
mod models;
mod routes;
mod server_fns;
mod ws;

use routes::Route;
```

With:

```rust
use dioxus::prelude::*;

mod components;
#[cfg(feature = "server")]
mod db;
mod models;
mod routes;
mod server_fns;
mod ws;

use routes::Route;
```

- [ ] **Step 2: Gate `parse_data_dir` and `build_server_router` on `feature = "server"`**

Replace lines 13–19 (`parse_data_dir`):

```rust
#[cfg(not(target_arch = "wasm32"))]
fn parse_data_dir() -> Option<String> {
```

With:

```rust
#[cfg(feature = "server")]
fn parse_data_dir() -> Option<String> {
```

`build_server_router` is already gated on `feature = "server"` (line 21) — leave it.

- [ ] **Step 3: Gate the tracing-subscriber / data-dir block in `main` on `feature = "server"`**

Replace the block at lines 31–46 starting `#[cfg(not(target_arch = "wasm32"))]` with:

```rust
    #[cfg(feature = "server")]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("lets_chat=info")),
            )
            .init();

        let data_dir = parse_data_dir()
            .or_else(|| std::env::var("LETS_CHAT_DATA_DIR").ok())
            .unwrap_or_else(|| "/data".to_string());
        tracing::info!(data_dir = %data_dir, "starting lets-chat");
        db::set_data_dir(data_dir);
    }
```

- [ ] **Step 4: Replace the whole `#[cfg(feature = "desktop")]` block with a client-only launcher**

Replace lines 64–91 (the entire `#[cfg(feature = "desktop")]` block starting with the comment `// Desktop: spawn an embedded Axum server…`) with:

```rust
    // Desktop client: launch the native UI only. The server URL is read from
    // LETS_CHAT_SERVER_URL (TODO: wire this into Dioxus's client-side server
    // function dispatcher in a follow-up).
    #[cfg(feature = "client")]
    {
        if let Ok(url) = std::env::var("LETS_CHAT_SERVER_URL") {
            eprintln!("lets-chat desktop: server URL = {url}");
        } else {
            eprintln!(
                "lets-chat desktop: LETS_CHAT_SERVER_URL not set; server calls will fail"
            );
        }

        dioxus::LaunchBuilder::new()
            .with_cfg(desktop! {
                dioxus::desktop::Config::new().with_window(
                    dioxus::desktop::WindowBuilder::new()
                        .with_always_on_top(false)
                )
            })
            .launch(App);
    }
```

- [ ] **Step 5: Update the web-server branch cfg predicate**

Replace lines 94–98:

```rust
    #[cfg(all(
        not(target_arch = "wasm32"),
        not(feature = "desktop"),
        feature = "server"
    ))]
```

With:

```rust
    #[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
```

(Desktop no longer has `feature = "server"`, so the extra `not(feature = "desktop")` guard is redundant.)

- [ ] **Step 6: Verify web build compiles**

Run: `cargo check --features server --no-default-features`
Expected: compiles.

- [ ] **Step 7: Verify client build compiles**

Run: `cargo check --features client --no-default-features`
Expected: compiles. If it fails because a component still references `crate::db::` or `crate::ws::handler::` directly (outside a server fn), add a `#[cfg(feature = "server")]` gate at that call site and re-run.

- [ ] **Step 8: Verify WASM build compiles**

Run: `cargo check --target wasm32-unknown-unknown --no-default-features`
Expected: compiles.

- [ ] **Step 9: Commit**

```bash
git add src/main.rs
git commit -m "refactor(main): split server and client entry points"
```

---

### Task 4: Confirm the `client` build has no server deps in its graph

**Files:** none (verification only).

- [ ] **Step 1: Print dep tree for client build**

Run: `cargo tree --features client --no-default-features --edges normal | grep -E '^(sqlx|axum|argon2|tracing-subscriber|dashmap) ' || echo "OK: no server deps"`
Expected: `OK: no server deps`.

If any of those crates appear, a dep is still unconditionally included. Go back and make it optional + add to the `server` feature list in Task 1.

- [ ] **Step 2: No commit (nothing changed)**

---

### Task 5: Update `justfile` recipes

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Update `dev-desktop` to use the renamed feature**

Replace:

```nu
# Start development server (desktop)
dev-desktop:
    dx serve --platform desktop
```

With:

```nu
# Start development server (desktop client)
dev-desktop:
    dx serve --platform desktop --features client --no-default-features
```

- [ ] **Step 2: Audit other recipes for any `--features desktop` references**

Run (as a sanity check): `grep -n desktop justfile`
Expected: only recipe names (`dev-desktop`, `check-desktop-linux`) and helpful echo strings — no `--features desktop` flags. If any exist, replace with `--features client`.

- [ ] **Step 3: Run `just check`**

Run: `just check`
Expected: `check-server`, `check-web`, `check-clippy`, `check-fmt` all pass.

Note: `just check` only exercises the default (server-less) cargo invocations. Add a recipe for the client check in Step 4.

- [ ] **Step 4: Add a `check-client` recipe and wire it into `check`**

In `justfile`, after `check-web`, add:

```nu
# Check client (desktop, native) compilation
check-client:
    cargo check --features client --no-default-features
```

Update the `check` aggregate recipe:

Replace:

```nu
check: check-server check-web check-clippy check-fmt
```

With:

```nu
check: check-server check-web check-client check-clippy check-fmt
```

Also update `check-server` to be explicit about using the server feature:

Replace:

```nu
# Check server compilation
check-server:
    cargo check
```

With:

```nu
# Check server compilation
check-server:
    cargo check --features server --no-default-features
```

- [ ] **Step 5: Run `just check`**

Run: `just check`
Expected: all four checks pass.

- [ ] **Step 6: Commit**

```bash
git add justfile
git commit -m "chore(justfile): add check-client, update dev-desktop for client feature"
```

---

### Task 6: Update Dockerfiles

**Files:**
- Modify: `ci-build/Dockerfile.desktop-linux`
- Modify: `ci-build/Dockerfile.web`

- [ ] **Step 1: Inspect both Dockerfiles for `--features desktop` or `--features server`**

Run: `grep -n features ci-build/Dockerfile.*`

- [ ] **Step 2: In `ci-build/Dockerfile.desktop-linux`, replace any `--features desktop` with `--features client --no-default-features`**

Use Edit with the exact matched string from Step 1's output.

- [ ] **Step 3: In `ci-build/Dockerfile.web`, ensure any cargo/dx invocation uses the server feature explicitly**

If the file invokes `cargo build` or `dx build --platform web` without `--features server`, add `--features server --no-default-features`. If it already uses the default (feature-less) build, leave it — `dx build --platform web` should still work because the plain `cargo check` works without explicit features in a pure-WASM target.

Note: it's fine to leave `Dockerfile.web` unchanged if the existing commands are not feature-scoped.

- [ ] **Step 4: Validate both builds**

Run: `just check-docker`
Expected: web Docker image builds successfully.

Run: `just check-desktop-linux`
Expected: desktop Docker image builds (may be runtime-broken — only compile-success is required).

- [ ] **Step 5: Commit**

```bash
git add ci-build/
git commit -m "chore(docker): update feature flags for server/client split"
```

---

### Task 7: End-to-end verification

**Files:** none.

- [ ] **Step 1: Run full test suite**

Run: `just test`
Expected: all tests pass.

- [ ] **Step 2: Verify the web binary still serves HTTP 200**

Run: `just verify`
Expected: `PASS: Server responded with HTTP 200`.

- [ ] **Step 3: Build the client binary**

Run: `cargo build --features client --no-default-features`
Expected: success. Runtime behavior is out of scope.

- [ ] **Step 4: Run formatting and clippy**

Run: `just fmt && just check`
Expected: clean.

- [ ] **Step 5: No commit (verification only)**

---

## Self-Review Notes

- **Spec coverage:** feature rename (Task 1), dep gating (Task 1), cfg-gate flips (Task 2), main.rs rewrite (Task 3), justfile (Task 5), Docker (Task 6), verification (Tasks 4 + 7). All spec sections covered.
- **Known TODOs carried forward:** wiring the client's `LETS_CHAT_SERVER_URL` into Dioxus's server-fn dispatcher is deliberately deferred (spec non-goal). Task 3 Step 4 leaves a `// TODO` comment plus a stderr print so the gap is visible.
- **Placeholder scan:** no TBDs in actionable steps; the one TODO is an intentional follow-up flagged in the spec.
- **Type consistency:** no new types introduced.
