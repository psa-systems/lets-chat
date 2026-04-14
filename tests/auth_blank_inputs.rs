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
