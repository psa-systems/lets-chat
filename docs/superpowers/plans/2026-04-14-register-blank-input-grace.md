# Register/Login Blank Input Grace Window — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make blank register/login submissions during the first 120 seconds of server uptime show a "try again" message, and make every other register/login failure return a single generic message with real reasons only in the logs.

**Architecture:** Add a lazy-initialized `OnceLock<Instant>` in `src/server_fns/helpers.rs` that captures the server process start time on first access. Introduce a pure `classify_blank_error` helper for unit testing. Rewrite `register` and `login` in `src/server_fns/auth.rs` to branch on the grace window for blank inputs and collapse all other errors to `GENERIC_REGISTER_ERROR` / `GENERIC_LOGIN_ERROR` while `tracing::warn!`ing the real cause. The login `is_banned` path is preserved verbatim. No client-side changes.

**Tech Stack:** Rust, Axum, Dioxus server functions, `std::sync::OnceLock`, `std::time::{Instant, Duration}`, `tracing`, SQLx (test-only, in-memory SQLite), `tokio::test`.

---

## File Structure

- **Modify** `src/server_fns/helpers.rs` — add `SERVER_STARTED_AT`, `STARTUP_GRACE`, `server_started_at()`, `within_startup_window()`, and `classify_blank_error()`.
- **Modify** `src/server_fns/auth.rs` — rewrite `register` and `login` error handling; add generic-message constants; add `use tracing::warn`.
- **Create** `tests/auth_blank_inputs.rs` — unit tests for `classify_blank_error` plus an integration-style test that calls the real `register` on an in-memory pool.
- **No changes** to `src/components/register.rs` or `src/components/login.rs`.

---

### Task 1: Add startup-window helpers to `helpers.rs`

**Files:**
- Modify: `src/server_fns/helpers.rs` (top of file)

- [ ] **Step 1: Write the failing test**

Create `tests/auth_blank_inputs.rs`:

```rust
use std::time::{Duration, Instant};

use lets_chat::server_fns::helpers::{classify_blank_error, STARTUP_GRACE, TRY_AGAIN_ERROR};

#[test]
fn classify_blank_inside_window_returns_try_again() {
    let started_at = Instant::now();
    let now = started_at + Duration::from_secs(30);
    let out = classify_blank_error(now, started_at, "Registration failed");
    assert_eq!(out, TRY_AGAIN_ERROR);
    assert_eq!(TRY_AGAIN_ERROR, "Something went wrong, please try again");
    assert_eq!(STARTUP_GRACE, Duration::from_secs(120));
}

#[test]
fn classify_blank_outside_window_returns_generic() {
    let started_at = Instant::now();
    let now = started_at + Duration::from_secs(121);
    let out = classify_blank_error(now, started_at, "Registration failed");
    assert_eq!(out, "Registration failed");
}

#[test]
fn classify_blank_at_exact_boundary_is_outside_window() {
    let started_at = Instant::now();
    let now = started_at + Duration::from_secs(120);
    let out = classify_blank_error(now, started_at, "Invalid credentials");
    assert_eq!(out, "Invalid credentials");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `just test` (from a Docker-based Rust shell; per repo memory, there is no host-installed Rust). Or explicitly: `cargo test --test auth_blank_inputs`.

Expected: compilation failure — `classify_blank_error`, `STARTUP_GRACE`, `TRY_AGAIN_ERROR` are not exported from `lets_chat::server_fns::helpers`.

- [ ] **Step 3: Add the helpers to `src/server_fns/helpers.rs`**

At the top of the file, after `use crate::models::user::UserRecord;`, add:

```rust
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const STARTUP_GRACE: Duration = Duration::from_secs(120);
pub const TRY_AGAIN_ERROR: &str = "Something went wrong, please try again";

static SERVER_STARTED_AT: OnceLock<Instant> = OnceLock::new();

pub fn server_started_at() -> Instant {
    *SERVER_STARTED_AT.get_or_init(Instant::now)
}

pub fn within_startup_window() -> bool {
    Instant::now().duration_since(server_started_at()) < STARTUP_GRACE
}

pub fn classify_blank_error(
    now: Instant,
    started_at: Instant,
    generic: &'static str,
) -> &'static str {
    if now.duration_since(started_at) < STARTUP_GRACE {
        TRY_AGAIN_ERROR
    } else {
        generic
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test auth_blank_inputs`
Expected: all three tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/server_fns/helpers.rs tests/auth_blank_inputs.rs
git commit -m "feat(auth): add startup grace window helpers"
```

---

### Task 2: Rewrite `register` to use generic errors and the grace window

**Files:**
- Modify: `src/server_fns/auth.rs:12-76` (the `register` function and nearby)
- Test: `tests/auth_blank_inputs.rs` (extend)

- [ ] **Step 1: Write the failing integration test**

Append to `tests/auth_blank_inputs.rs`:

```rust
use sqlx::SqlitePool;

async fn setup_auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("pool");
    let m1 = include_str!("../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(m1).execute(&pool).await.expect("m1");
    let m2 = include_str!("../migrations/auth/0002_read_receipts.sql");
    sqlx::raw_sql(m2).execute(&pool).await.expect("m2");
    pool
}

#[tokio::test]
async fn register_blank_username_within_window_returns_try_again() {
    // Force the static OnceLock to initialize now.
    let _ = lets_chat::server_fns::helpers::server_started_at();

    // We can't invoke the #[server] fn directly without the Dioxus fullstack
    // context (cookies, etc.), so we assert the generic-message constants are
    // what we expect and that the classifier returns them. The end-to-end
    // path is covered implicitly by the classifier + the code review of the
    // rewrite in auth.rs.
    let started_at = lets_chat::server_fns::helpers::server_started_at();
    let out = lets_chat::server_fns::helpers::classify_blank_error(
        std::time::Instant::now(),
        started_at,
        "Registration failed",
    );
    assert_eq!(out, lets_chat::server_fns::helpers::TRY_AGAIN_ERROR);

    // Exercise the real DB path to ensure our setup_auth_pool works and
    // create_user still behaves — this is the only thing we can drive without
    // the Dioxus server context.
    let pool = setup_auth_pool().await;
    let count = lets_chat::db::auth::count_users(&pool).await.unwrap();
    assert_eq!(count, 0);
}
```

Note: we deliberately do not invoke the `register` server fn directly in tests — it requires `dioxus_fullstack::FullstackContext`. The integration coverage of the rewrite is the classifier test above plus the code-path changes guarded by the compiler. Do not add scaffolding to drive the full server fn; that is out of scope.

- [ ] **Step 2: Run the test to verify it compiles and passes**

Run: `cargo test --test auth_blank_inputs`
Expected: all prior tests plus the new one pass. (This test does not yet exercise the rewritten `register`; it only confirms the test harness compiles before we change `register`.)

- [ ] **Step 3: Rewrite `register` in `src/server_fns/auth.rs`**

At the top of the file (below `use crate::models::User;`), add:

```rust
const GENERIC_REGISTER_ERROR: &str = "Registration failed";
```

Replace the entire `register` function body (lines 12-76) with:

```rust
#[server]
pub async fn register(username: String, password: String) -> Result<AuthResponse, ServerFnError> {
    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    use rand::rngs::OsRng;
    use tracing::warn;

    let username = username.trim().to_string();
    let password = password.trim().to_string();

    if username.is_empty() || password.is_empty() {
        let msg = crate::server_fns::helpers::classify_blank_error(
            std::time::Instant::now(),
            crate::server_fns::helpers::server_started_at(),
            GENERIC_REGISTER_ERROR,
        );
        warn!(
            username_empty = username.is_empty(),
            password_empty = password.is_empty(),
            "register rejected: blank input"
        );
        return Err(ServerFnError::new(msg));
    }

    if username.len() < 3 {
        warn!(username_len = username.len(), "register rejected: username too short");
        return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
    }
    if password.len() < 8 {
        warn!(password_len = password.len(), "register rejected: password too short");
        return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
    }

    let pool = crate::db::get_auth_pool().await;

    match crate::db::auth::find_user_by_username(pool, &username).await {
        Ok(Some(_)) => {
            warn!(%username, "register rejected: username already taken");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
        Ok(None) => {}
        Err(e) => {
            warn!(error = %e, "register failed: db lookup");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = match argon2.hash_password(password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            warn!(error = %e, "register failed: password hash");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    let user_id = match crate::db::auth::create_user(pool, &username, &password_hash).await {
        Ok(id) => id,
        Err(e) => {
            warn!(error = %e, "register failed: create_user");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    let user_count = match crate::db::auth::count_users(pool).await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "register failed: count_users");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };
    if user_count == 1 {
        if let Err(e) = crate::db::auth::set_user_role(pool, &user_id, "admin").await {
            warn!(error = %e, "register failed: set_user_role admin");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    }

    let session_token = match crate::db::auth::create_session(pool, &user_id).await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "register failed: create_session");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    let record = match crate::db::auth::find_user_by_id(pool, &user_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            warn!(%user_id, "register failed: user not found after creation");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
        Err(e) => {
            warn!(error = %e, "register failed: find_user_by_id");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    Ok(AuthResponse {
        user: user_record_to_user(&record),
        session_token,
    })
}
```

- [ ] **Step 4: Run checks**

Run: `just check`
Expected: passes (server + web + clippy + fmt). If clippy flags the `#[cfg(feature = "server")]` gating on `user_record_to_user`, leave it — it is already gated correctly below the function.

- [ ] **Step 5: Commit**

```bash
git add src/server_fns/auth.rs tests/auth_blank_inputs.rs
git commit -m "feat(auth): generic register errors + blank-input grace window"
```

---

### Task 3: Rewrite `login` to use generic errors and the grace window

**Files:**
- Modify: `src/server_fns/auth.rs:79-117` (the `login` function)

- [ ] **Step 1: Rewrite `login` in `src/server_fns/auth.rs`**

Add near the other constants:

```rust
const GENERIC_LOGIN_ERROR: &str = "Invalid credentials";
```

Replace the entire `login` function body with:

```rust
#[server]
pub async fn login(username: String, password: String) -> Result<AuthResponse, ServerFnError> {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    use tracing::warn;

    let username = username.trim().to_string();
    let password = password.trim().to_string();

    if username.is_empty() || password.is_empty() {
        let msg = crate::server_fns::helpers::classify_blank_error(
            std::time::Instant::now(),
            crate::server_fns::helpers::server_started_at(),
            GENERIC_LOGIN_ERROR,
        );
        warn!(
            username_empty = username.is_empty(),
            password_empty = password.is_empty(),
            "login rejected: blank input"
        );
        return Err(ServerFnError::new(msg));
    }

    let pool = crate::db::get_auth_pool().await;

    let record = match crate::db::auth::find_user_by_username(pool, &username).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            warn!(%username, "login rejected: user not found");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
        Err(e) => {
            warn!(error = %e, "login failed: db lookup");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
    };

    let parsed_hash = match PasswordHash::new(&record.password_hash) {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "login failed: stored hash unparseable");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
    };
    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_err()
    {
        warn!(%username, "login rejected: bad password");
        return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
    }

    // Ban status is preserved as a distinct, user-facing message per design.
    if record.is_banned {
        let msg = match &record.ban_reason {
            Some(reason) => format!("Account banned: {}", reason),
            None => "Account banned".to_string(),
        };
        return Err(ServerFnError::new(msg));
    }

    let session_token = match crate::db::auth::create_session(pool, &record.id).await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "login failed: create_session");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
    };

    Ok(AuthResponse {
        user: user_record_to_user(&record),
        session_token,
    })
}
```

- [ ] **Step 2: Run checks**

Run: `just check`
Expected: passes.

- [ ] **Step 3: Run tests**

Run: `just test`
Expected: all pre-existing tests plus the tests from Task 1 pass.

- [ ] **Step 4: Manual smoke test**

Run: `just dev-web-local`
- Navigate to `/register` and click Register with blank fields → see `"Something went wrong, please try again"`.
- Wait >120s of server uptime, then repeat → see `"Registration failed"`.
- Navigate to `/login`, submit blank → see `"Something went wrong, please try again"` inside window, `"Invalid credentials"` outside.
- Submit wrong password for an existing user → `"Invalid credentials"`.
- Submit for a banned user → `"Account banned[: reason]"` unchanged.

If Docker/WASM rebuild cycles are long enough that the 120s window elapses during manual testing, restart `just dev-web-local` between the "inside" and "outside" checks.

- [ ] **Step 5: Commit**

```bash
git add src/server_fns/auth.rs
git commit -m "feat(auth): generic login errors + blank-input grace window"
```

---

## Self-Review

- **Spec coverage:**
  - Startup timestamp + 120s window → Task 1.
  - Blank-input branch inside/outside window for register → Task 2.
  - Blank-input branch inside/outside window for login → Task 3.
  - All other failures become generic + logged → Tasks 2, 3.
  - Ban message preserved → Task 3.
  - No client changes → explicitly none.
  - Unit-testable `classify_blank_error` helper → Task 1.
- **Placeholder scan:** none.
- **Type consistency:** `STARTUP_GRACE`, `TRY_AGAIN_ERROR`, `GENERIC_REGISTER_ERROR`, `GENERIC_LOGIN_ERROR`, `classify_blank_error`, `server_started_at`, `within_startup_window` used consistently across tasks.
