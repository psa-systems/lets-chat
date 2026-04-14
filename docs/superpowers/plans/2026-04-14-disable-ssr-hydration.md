# Disable SSR / Switch Web Build to CSR Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the register/login hydration-race bugs by removing Dioxus SSR and running the web app as a pure CSR (client-side rendered) single-page app. Server functions continue to work as HTTP endpoints.

**Architecture:** The backend Axum router stops pre-rendering HTML via `serve_dioxus_application`. Instead, it registers server-function endpoints, serves static WASM/JS assets, and serves a bare `index.html` shell for all client routes. The browser downloads the WASM bundle and renders from scratch — no hydration handoff, no "pre-hydration interactive" window. Login/register components collapse back to plain controlled signals with no DOM reads or hydration guards.

**Tech Stack:** Dioxus 0.7 (router, fullstack feature kept for `#[server]` fn codegen), Axum 0.8, existing SQLx + WebSocket infrastructure unchanged.

---

## Non-goals

- Desktop build behavior (preserved as-is).
- Server function *signatures* or DB layer (unchanged).
- Any component outside `login.rs` / `register.rs` (they'll benefit automatically but needn't be edited).

## Files touched

- Modify: `src/main.rs` — replace `serve_dioxus_application` with CSR-compatible routing.
- Modify: `src/components/register.rs` — drop hydration guards + DOM reads.
- Modify: `src/components/login.rs` — drop hydration guards + DOM reads.
- Possibly modify: `Cargo.toml` — adjust features if the Dioxus CSR path requires it.
- Possibly modify: `justfile` / `Dockerfile` — only if build commands change.

---

### Task 1: Research spike — determine Dioxus 0.7 CSR wiring

**Files:** none yet (research only).

We need to know the exact Dioxus 0.7.3 API to: (a) register `#[server]` function endpoints on an Axum router without SSR, and (b) serve WASM + `index.html` from the same router. The current code uses `DioxusRouterExt::serve_dioxus_application` which bundles both SSR and server-fn registration.

- [ ] **Step 1: Inspect installed Dioxus source for the router-extension methods**

Run (inside the project's dev container / shell with cargo available):

```bash
cargo doc --no-deps --package dioxus-fullstack --open
# or, without opening a browser:
find ~/.cargo/registry/src -path '*dioxus-fullstack*' -name '*.rs' | xargs grep -l 'register_server_functions\|serve_dioxus_application\|serve_static_assets' 2>/dev/null
```

Record the actual method names available on `DioxusRouterExt` in Dioxus 0.7.3. Likely candidates:
- `register_server_functions()` — mounts only the server-fn POST endpoints
- `serve_static_assets()` — mounts `/assets` (and possibly the wasm bundle)
- A method that serves a generated `index.html` shell without SSR

- [ ] **Step 2: Confirm `dx build` output layout**

Run:

```bash
just dev-web-local
# while running, in another shell:
ls -la target/dx/lets-chat/release/web/public/ 2>/dev/null || \
  find target -name 'index.html' -path '*web*' 2>/dev/null
```

Identify where the built `index.html`, WASM blob, and JS glue live after `dx build`. We need the path to hand to a static file service.

- [ ] **Step 3: Write a short findings note**

Append to this plan file under a new `## Research findings` heading with the method names chosen and the asset path. No code yet.

- [ ] **Step 4: Commit the findings**

```bash
git add docs/superpowers/plans/2026-04-14-disable-ssr-hydration.md
git commit -m "docs(plan): record Dioxus CSR research findings"
```

---

### Task 2: Switch the server router from SSR to CSR

**Files:**
- Modify: `src/main.rs:21-28` (`build_server_router`)

- [ ] **Step 1: Replace `serve_dioxus_application` with server-fn + static + shell**

Using the method names identified in Task 1, rewrite `build_server_router` so it:
1. Keeps the `/ws` route.
2. Registers only the server-function endpoints (no SSR renderer).
3. Serves the built WASM assets directory.
4. Falls back to serving `index.html` for any unmatched GET (so client-side routes like `/login`, `/register`, `/rooms/:id` all hit the SPA shell).

Pseudocode (fill in actual method names from research):

```rust
#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
fn build_server_router() -> axum::Router {
    use axum::routing::get;
    use dioxus::server::DioxusRouterExt;

    axum::Router::new()
        .route("/ws", get(ws::handler::ws_handler))
        .register_server_functions()               // <-- from research
        .serve_static_assets()                     // <-- from research; or tower_http::ServeDir
        .fallback(/* serve index.html */)          // <-- from research
}
```

If `DioxusRouterExt` in 0.7 does not expose a standalone `index.html` fallback, use `tower_http::services::ServeFile` pointing at the built shell path from Task 1.

- [ ] **Step 2: Build the release server binary**

```bash
just build
```

Expected: compiles cleanly, Tailwind rebuilds, release binary produced.

- [ ] **Step 3: Run dev server and smoke-test**

```bash
just dev-web-local
```

In a browser: open http://localhost:8080. Verify:
- The page loads (may briefly show a blank shell while WASM downloads — that's expected and correct behavior now).
- View source shows a minimal shell, **not** server-rendered component HTML.
- Navigating to `/login` and `/register` both reach the SPA (client router handles them).

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "refactor(web): serve CSR shell + server fns instead of SSR"
```

---

### Task 3: Simplify `register.rs` — remove hydration guards and DOM reads

**Files:**
- Modify: `src/components/register.rs` (full rewrite of the component body; delete `read_input_value`)

- [ ] **Step 1: Rewrite the file**

Replace the entire contents with:

```rust
use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn RegisterPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();

    let do_register = move || {
        if loading() {
            return;
        }
        let u = username();
        let p = password();
        let cp = confirm_password();
        if p != cp {
            error.set(Some("Passwords do not match".to_string()));
            return;
        }
        error.set(None);
        loading.set(true);
        spawn(async move {
            match auth::register(u, p).await {
                Ok(resp) => {
                    auth::set_session_cookie(&resp.session_token);
                    nav.push(Route::Home {});
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                }
            }
        });
    };

    let button_class = if loading() {
        "w-full bg-blue-400 text-white py-2 rounded opacity-50 cursor-not-allowed"
    } else {
        "w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700"
    };
    let button_label = if loading() { "Creating account..." } else { "Register" };

    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-gray-100",
            div { class: "bg-white p-8 rounded-lg shadow-md w-full max-w-sm",
                h1 { class: "text-2xl font-bold text-center mb-1", "Let's Chat" }
                p { class: "text-gray-500 text-center mb-6", "Create an account" }

                if let Some(err) = error() {
                    div { class: "bg-red-50 text-red-700 p-3 rounded mb-4 text-sm", "{err}" }
                }

                div {
                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "username", "Username" }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            id: "username",
                            value: "{username}",
                            oninput: move |evt| username.set(evt.value()),
                            onkeydown: move |evt| { if evt.key() == Key::Enter { do_register(); } },
                        }
                    }

                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "password", "Password" }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            id: "password",
                            value: "{password}",
                            oninput: move |evt| password.set(evt.value()),
                            onkeydown: move |evt| { if evt.key() == Key::Enter { do_register(); } },
                        }
                    }

                    div { class: "mb-6",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "confirm_password", "Confirm password" }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            id: "confirm_password",
                            value: "{confirm_password}",
                            oninput: move |evt| confirm_password.set(evt.value()),
                            onkeydown: move |evt| { if evt.key() == Key::Enter { do_register(); } },
                        }
                    }

                    button {
                        class: button_class,
                        r#type: "button",
                        disabled: loading(),
                        onclick: move |_| do_register(),
                        "{button_label}"
                    }
                }

                p { class: "mt-4 text-center text-sm text-gray-500",
                    "Already have an account? "
                    Link { class: "text-blue-600 hover:underline", to: Route::Login {}, "Sign in" }
                }
            }
        }
    }
}
```

Key changes from the old version:
- No `hydrated` signal, no `use_hook` spawn, no `read_input_value`, no `#[cfg(target_arch = "wasm32")]` imports of `web_sys`/`wasm_bindgen`.
- `oninput` writes directly to signals; submit reads signals.
- Button disabled only while `loading()`.

- [ ] **Step 2: Build**

```bash
just check
```

Expected: clippy + fmt clean, no warnings about unused `web_sys` / `wasm_bindgen` imports.

- [ ] **Step 3: Commit**

```bash
git add src/components/register.rs
git commit -m "refactor(auth): drop hydration guards from register page"
```

---

### Task 4: Simplify `login.rs` — mirror register changes

**Files:**
- Modify: `src/components/login.rs`

- [ ] **Step 1: Rewrite the file**

Replace contents with the login analogue (one fewer field, "Sign in" labels, matching structure). Follow the same pattern as Task 3: plain signals, no hydrated guard, no DOM reads.

```rust
use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn LoginPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();

    let do_login = move || {
        if loading() {
            return;
        }
        let u = username();
        let p = password();
        error.set(None);
        loading.set(true);
        spawn(async move {
            match auth::login(u, p).await {
                Ok(resp) => {
                    auth::set_session_cookie(&resp.session_token);
                    nav.push(Route::Home {});
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                }
            }
        });
    };

    let button_class = if loading() {
        "w-full bg-blue-400 text-white py-2 rounded opacity-50 cursor-not-allowed"
    } else {
        "w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700"
    };
    let button_label = if loading() { "Signing in..." } else { "Sign in" };

    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-gray-100",
            div { class: "bg-white p-8 rounded-lg shadow-md w-full max-w-sm",
                h1 { class: "text-2xl font-bold text-center mb-1", "Let's Chat" }
                p { class: "text-gray-500 text-center mb-6", "Sign in" }

                if let Some(err) = error() {
                    div { class: "bg-red-50 text-red-700 p-3 rounded mb-4 text-sm", "{err}" }
                }

                div { class: "mb-4",
                    label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "username", "Username" }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                        r#type: "text",
                        id: "username",
                        value: "{username}",
                        oninput: move |evt| username.set(evt.value()),
                        onkeydown: move |evt| { if evt.key() == Key::Enter { do_login(); } },
                    }
                }

                div { class: "mb-6",
                    label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "password", "Password" }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                        r#type: "password",
                        id: "password",
                        value: "{password}",
                        oninput: move |evt| password.set(evt.value()),
                        onkeydown: move |evt| { if evt.key() == Key::Enter { do_login(); } },
                    }
                }

                button {
                    class: button_class,
                    r#type: "button",
                    disabled: loading(),
                    onclick: move |_| do_login(),
                    "{button_label}"
                }

                p { class: "mt-4 text-center text-sm text-gray-500",
                    "Don't have an account? "
                    Link { class: "text-blue-600 hover:underline", to: Route::Register {}, "Register" }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Build**

```bash
just check
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/components/login.rs
git commit -m "refactor(auth): drop hydration guards from login page"
```

---

### Task 5: End-to-end verification

**Files:** none modified.

- [ ] **Step 1: Cold-start reproduction**

Stop any running dev server. Start fresh:

```bash
just dev-web-down 2>/dev/null || true
just dev-web-local
```

In a private / freshly cleared browser window, the instant the server logs "listening", navigate to `/register`. Confirm:
1. Page is briefly blank or shows a flash of unstyled content while WASM downloads (acceptable, and is the **entire point** — the user can no longer interact with a non-functional form).
2. Once the app appears, clicking Register works immediately — no ~2-minute "dead button" period.
3. Immediately typing and clicking Register after the form appears yields a real registration attempt (no "empty fields" error unless fields are actually empty).

- [ ] **Step 2: Login happy path**

Register a user, log out (clear cookie), log back in. Confirm successful redirect to `Home`.

- [ ] **Step 3: Run the test suite**

```bash
just test
```

Expected: all tests pass (these test server logic only; no UI tests should be affected).

- [ ] **Step 4: Run the production build verification**

```bash
just verify
```

Expected: release binary builds, HTTP 200 from a smoke probe.

- [ ] **Step 5: Final commit (if any cleanup)**

```bash
git status
# If clean, nothing to do. Otherwise commit any small fixups.
```

---

## Rollback

If Task 2 breaks server-function routing and Task 1 research was wrong:

```bash
git revert <commit-sha-of-task-2>
```

Tasks 3 and 4 are safe to keep even under SSR — simpler code does not reintroduce the hydration race because it doesn't do anything different; the race is a property of the server, not the component.

## Success criteria

- No more `hydrated` / `read_input_value` machinery in the auth components.
- A cold server start followed by an immediate register attempt works without the "dead button" or "empty fields" error windows.
- All existing tests still pass.
- `just verify` returns HTTP 200.
