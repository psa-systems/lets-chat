# Register/Login Blank Input Grace Window — Design

## Problem

After the Register button becomes clickable, there is a short interval before Dioxus finishes wiring the `oninput` handlers on the WASM page. A click during that interval submits the form with empty `username` / `password` values. The server currently responds with specific validation messages such as `"Username must be at least 3 characters"`, which both confuses the user (who believes they typed a real username) and leaks implementation details.

The same failure mode applies to the login page.

## Goals

1. During a grace window after server startup, treat blank submissions as a transient error with a "try again" message.
2. Outside that window, still reject blank submissions — but with the same generic failure message used for every other register/login failure.
3. Make every register/login failure message generic. Real reasons go to the server logs, not the client.
4. One exception: the existing "Account banned: {reason}" message on login is kept verbatim. Being banned is a terminal UX state, not a security disclosure.

## Non-Goals

- Fixing the underlying hydration race. Shortening it is tracked separately; this spec only handles the user-visible symptom.
- Changing the client-side "Passwords do not match" check — pure form validation, no auth leak.
- Rate limiting, account lockout, or other abuse prevention.

## Design

### Server startup timestamp

Add to `src/server_fns/helpers.rs` (already `#[cfg(feature = "server")]`):

```rust
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static SERVER_STARTED_AT: OnceLock<Instant> = OnceLock::new();
const STARTUP_GRACE: Duration = Duration::from_secs(120);

pub fn server_started_at() -> Instant {
    *SERVER_STARTED_AT.get_or_init(Instant::now)
}

pub fn within_startup_window() -> bool {
    Instant::now().duration_since(server_started_at()) < STARTUP_GRACE
}
```

Lazy init means the clock starts on first auth attempt rather than precisely at server boot. That is acceptable: in practice the first auth attempt happens within seconds of boot, and on a fresh process `SERVER_STARTED_AT` is always unset, so the grace window always fires for the first 120 seconds of real auth traffic.

### Generic error constants

In `src/server_fns/auth.rs`:

```rust
const GENERIC_REGISTER_ERROR: &str = "Registration failed";
const GENERIC_LOGIN_ERROR: &str = "Invalid credentials";
const TRY_AGAIN_ERROR: &str = "Something went wrong, please try again";
```

### `register` rewrite

After trimming inputs:

1. If `username.is_empty() || password.is_empty()`:
   - `within_startup_window()` → return `TRY_AGAIN_ERROR`.
   - Else → return `GENERIC_REGISTER_ERROR`.
2. For every other failure path — length rules, username-taken, hash error, DB error, session creation, post-insert fetch — `tracing::warn!` the real reason and return `GENERIC_REGISTER_ERROR`.

### `login` rewrite

After trimming inputs:

1. If `username.is_empty() || password.is_empty()`:
   - `within_startup_window()` → return `TRY_AGAIN_ERROR`.
   - Else → return `GENERIC_LOGIN_ERROR`.
2. For user-not-found, bad-password, hash-parse, DB error, session creation — `tracing::warn!` the real reason and return `GENERIC_LOGIN_ERROR`.
3. For `record.is_banned` — keep the existing `"Account banned: {reason}"` / `"Account banned"` messages unchanged.

### Client

No changes to `src/components/register.rs` or `src/components/login.rs`. Both already surface `e.to_string()` from `ServerFnError`, which will now carry the generic text.

### Testability

The grace check is built from two small pieces: `within_startup_window()` and the branch inside each server fn. The server functions themselves are hard to unit test (they need a DB pool and macro expansion), so the unit-testable piece is extracted:

```rust
pub(crate) fn classify_blank_error(
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

`within_startup_window()` becomes a one-line wrapper around `classify_blank_error(Instant::now(), server_started_at(), ...)` in the actual call sites (or calls the helper directly). Tests live in `tests/auth_blank_inputs.rs` and verify:

- `classify_blank_error` returns the "try again" text when `now - started_at < 120s`.
- It returns the generic text when `now - started_at >= 120s`.
- Integration test: calling `register("", "")` via the server fn returns the try-again text on a fresh pool (process is young, grace window active).

## Architecture Impact

Touches only `src/server_fns/helpers.rs` and `src/server_fns/auth.rs`. No schema changes, no new dependencies, no client changes. `OnceLock` and `Instant` are in `std`.

## Logging

All swallowed error details are emitted at `tracing::warn!` with a short message and the underlying error as a field. Existing log levels and subscribers in `main.rs` are unchanged.
