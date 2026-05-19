//! Fixed-window per-key rate limiter (LC-94).
//!
//! Keyed by `(RateLimitKind, key_string)`; the key is whatever the
//! caller decides identifies the actor (a user id, an IP, etc.). Each
//! key gets its own window: the first request seeds `window_start =
//! now()` and the count at 1; subsequent requests within `WINDOW`
//! increment the count; the count caps at the per-kind limit; once
//! `now - window_start >= WINDOW` the window resets.
//!
//! This is intentionally *not* a sliding window or token bucket. The
//! operator-facing setting is "N per minute"; that's what they get.
//! Bursts up to N are allowed inside the minute; the next minute
//! starts fresh. Simple to reason about, simple to test, simple to
//! tune.
//!
//! Storage is an `Arc<DashMap>`: shared across handler tasks, no
//! per-request lock. A stale-key sweep is not needed for normal
//! traffic - the map grows by one entry per unique (kind, key) and
//! stays modest for any self-hosted deployment - but the comment at
//! `should_touch_last_seen` in auth.rs about dropping the DashMap Ref
//! before calling insert applies here too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// Fixed window length for every kind. Matches the operator-facing
/// "N per minute" framing on the admin settings page.
pub const WINDOW: Duration = Duration::from_secs(60);

/// Categorical bucket so distinct routes do not share a counter when
/// they happen to key on the same string (e.g., the same IP hitting
/// `/register` and `/forgot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitKind {
    /// Per-user cap on `POST /room/{id}/messages`.
    Message,
    /// Per-IP cap on `POST /register`.
    Register,
    /// Per-IP cap on `POST /forgot`.
    PasswordReset,
}

impl RateLimitKind {
    fn tag(self) -> &'static str {
        match self {
            RateLimitKind::Message => "msg",
            RateLimitKind::Register => "reg",
            RateLimitKind::PasswordReset => "pwr",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WindowState {
    start: Instant,
    count: u32,
}

/// Outcome of a rate-limit check. `Allow` means the caller should
/// proceed; `Deny { retry_after }` means the cap is hit and the
/// caller should return 429 with a `Retry-After` header set to the
/// seconds remaining in the current window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Allow,
    Deny { retry_after: u64 },
}

#[derive(Clone, Default)]
pub struct RateLimits {
    map: Arc<DashMap<String, WindowState>>,
}

/// Read a `settings.db` KV value as `u32`. Missing keys, blank
/// values, and non-numeric values all collapse to `0`, which the
/// `check` method treats as "rate limiting disabled" - the
/// safe-by-default convention. Saturates rather than overflowing if
/// an admin types a number larger than `u32::MAX`.
pub async fn read_u32_setting(pool: &sqlx::SqlitePool, key: &str) -> u32 {
    crate::db::settings::get_setting(pool, key)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
}

impl RateLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check + increment in one shot. `limit_per_minute = 0` is
    /// interpreted as "rate limiting disabled" and always returns
    /// `Allow`; callers don't have to wrap the call in an extra
    /// "is enabled" check.
    pub fn check(&self, kind: RateLimitKind, key: &str, limit_per_minute: u32) -> Outcome {
        if limit_per_minute == 0 {
            return Outcome::Allow;
        }
        let composite = format!("{}:{}", kind.tag(), key);
        let now = Instant::now();
        let mut entry = self.map.entry(composite).or_insert(WindowState {
            start: now,
            count: 0,
        });
        if now.duration_since(entry.start) >= WINDOW {
            entry.start = now;
            entry.count = 0;
        }
        if entry.count >= limit_per_minute {
            let elapsed = now.duration_since(entry.start);
            let retry_after = WINDOW.saturating_sub(elapsed).as_secs().max(1);
            return Outcome::Deny { retry_after };
        }
        entry.count += 1;
        Outcome::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_limit_is_disabled() {
        let r = RateLimits::new();
        for _ in 0..100 {
            assert_eq!(r.check(RateLimitKind::Message, "alice", 0), Outcome::Allow);
        }
    }

    #[test]
    fn allows_up_to_limit_then_denies() {
        let r = RateLimits::new();
        for _ in 0..5 {
            assert_eq!(r.check(RateLimitKind::Message, "bob", 5), Outcome::Allow);
        }
        match r.check(RateLimitKind::Message, "bob", 5) {
            Outcome::Deny { retry_after } => assert!((1..=60).contains(&retry_after)),
            Outcome::Allow => panic!("expected deny after hitting cap"),
        }
    }

    #[test]
    fn different_keys_have_independent_counters() {
        let r = RateLimits::new();
        for _ in 0..2 {
            assert_eq!(r.check(RateLimitKind::Message, "x", 2), Outcome::Allow);
        }
        // y still has full headroom.
        assert_eq!(r.check(RateLimitKind::Message, "y", 2), Outcome::Allow);
    }

    #[test]
    fn different_kinds_do_not_share_counters() {
        let r = RateLimits::new();
        for _ in 0..2 {
            assert_eq!(r.check(RateLimitKind::Message, "z", 2), Outcome::Allow);
        }
        // Same key, different kind, still allowed.
        assert_eq!(r.check(RateLimitKind::Register, "z", 2), Outcome::Allow);
    }
}
