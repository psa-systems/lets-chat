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
//! per-request lock. The comment at `should_touch_last_seen` in
//! auth.rs about dropping the DashMap Ref before calling insert
//! applies here too.
//!
//! Stale-key sweep: every `SWEEP_EVERY` checks the map is walked once
//! and any entry whose window expired is dropped. A public `/register`
//! endpoint would otherwise accumulate one row per unique IP forever;
//! the sweep keeps memory bounded at roughly `O(active keys in the
//! last minute)`. The sweep itself is O(map size); it runs at most
//! once per N requests so the amortized cost stays flat.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// Fixed window length for every kind. Matches the operator-facing
/// "N per minute" framing on the admin settings page.
pub const WINDOW: Duration = Duration::from_secs(60);

/// How often to opportunistically prune expired entries. One sweep
/// per N rate-limit checks: high enough to amortize the walk cost,
/// low enough that a long-running process does not grow the map
/// indefinitely.
const SWEEP_EVERY: u64 = 1024;

/// Categorical bucket so distinct routes do not share a counter when
/// they happen to key on the same string (e.g., the same IP hitting
/// `/register` and `/forgot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitKind {
    /// Per-user cap on `POST /room/{id}/messages`.
    Message,
    /// Per-IP cap on `POST /register`.
    Register,
    /// LC-151: per-IP cap shared by `POST /login`, `POST /login/2fa`, and
    /// `POST /login/recovery` to blunt online password + code brute force.
    Login,
    /// Per-IP cap on `POST /forgot`.
    PasswordReset,
    /// LC-74: per-webhook cap on `POST /webhook/{secret}`.
    Webhook,
    /// LC-77: per-email-inbox cap. Keyed by the inbox row id. Counts inbound
    /// messages processed per minute by the IMAP poll loop; over-limit
    /// messages are dropped silently and logged.
    EmailInbox,
    /// LC-77-REPLY: per-user cap on outbound mention/DM notification emails.
    /// Keyed by recipient user_id. 20/minute (~ hourly cap on per-minute
    /// rate); over-limit emails drop silently with `Outcome::Deny` and the
    /// dispatcher returns SkippedRateLimit. A future
    /// LC-77-REPLY-COALESCE follow-up may replace this with a debounce.
    EmailMentionNotification,
    /// LC-186: per-requester cap on remote-control `request` signals. Keyed by
    /// the requesting user_id. Blunts request spam / re-request harassment;
    /// over-limit requests drop silently in `relay_control_signal`.
    RemoteControlRequest,
    /// LC-78: per-bridge cap on `POST /api/v1/bridges/{id}/messages`. Keyed
    /// by the bridges.id. Separate bucket from `Message` so bursty foreign
    /// traffic (federated Matrix rooms can be chatty) does not share a
    /// counter with human posting.
    BridgeMessage,
    /// LC-492: per-user cap on the in-channel AI assistant (`/ask`). Keyed by
    /// the asking user_id. Bounds LLM cost/load; over-limit asks are refused.
    AssistantAsk,
}

impl RateLimitKind {
    fn tag(self) -> &'static str {
        match self {
            RateLimitKind::Message => "msg",
            RateLimitKind::Register => "reg",
            RateLimitKind::Login => "login",
            RateLimitKind::PasswordReset => "pwr",
            RateLimitKind::Webhook => "whk",
            RateLimitKind::EmailInbox => "eml",
            RateLimitKind::EmailMentionNotification => "emn",
            RateLimitKind::RemoteControlRequest => "rcr",
            RateLimitKind::BridgeMessage => "brm",
            RateLimitKind::AssistantAsk => "ask",
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
    check_counter: Arc<AtomicU64>,
}

/// IP address to key a per-IP rate limit on, or `None` if the
/// deployment is not configured to trust client-IP headers (or if no
/// header is present). Pulls from `extract_session_origin` only when
/// the operator has opted in via `settings.trust_proxy_headers`,
/// because `X-Forwarded-For` is trivially spoofable on a server that
/// faces the internet directly. Falling back to `None` deliberately
/// no-ops the rate-limit check rather than keying on a spoofable
/// value: callers treat `None` as "skip the limit", which is the
/// safer default than silently blessing whatever the client sent.
pub async fn client_ip_for_rate_limit(
    pool: &sqlx::SqlitePool,
    headers: &axum::http::HeaderMap,
) -> Option<String> {
    if !crate::auth::proxy_headers_trusted(pool).await {
        return None;
    }
    // Trust already confirmed; extract the proxy-supplied client IP.
    crate::auth::extract_session_origin(headers, true).1
}

/// Read a `settings.db` KV value as `u32`. Missing keys and blank
/// values collapse to `0` (rate limiting disabled) silently; a
/// non-numeric value logs a WARN before falling back so a typo in the
/// admin UI does not invisibly disable the cap.
pub async fn read_u32_setting(pool: &sqlx::SqlitePool, key: &str) -> u32 {
    let raw = crate::db::settings::get_setting(pool, key)
        .await
        .ok()
        .flatten();
    let Some(v) = raw else { return 0 };
    let trimmed = v.trim();
    if trimmed.is_empty() {
        return 0;
    }
    match trimmed.parse::<u32>() {
        Ok(n) => n,
        Err(_) => {
            tracing::warn!(
                key,
                value = trimmed,
                "rate-limit setting is not a non-negative integer; treating as disabled"
            );
            0
        }
    }
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
        // Opportunistic GC: every N checks, prune entries whose
        // windows have expired. Bounds memory on long-running
        // deployments without a separate background task.
        if self
            .check_counter
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_EVERY)
        {
            self.sweep_expired();
        }
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

    /// Drop entries whose windows have fully elapsed. Public so a
    /// future background task can call it on a schedule, but the
    /// opportunistic call inside `check` is sufficient for normal
    /// traffic.
    pub fn sweep_expired(&self) {
        let now = Instant::now();
        self.map
            .retain(|_, state| now.duration_since(state.start) < WINDOW);
    }

    /// Current entry count. Exposed for tests; production code has
    /// no reason to introspect.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
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

    #[test]
    fn sweep_drops_expired_entries() {
        let r = RateLimits::new();
        // Seed an entry with a fake stale window by going through
        // the public API once, then manually backdating its start.
        r.check(RateLimitKind::Message, "old", 5);
        assert_eq!(r.len(), 1);
        {
            let key = format!("{}:old", RateLimitKind::Message.tag());
            let mut entry = r.map.get_mut(&key).unwrap();
            entry.start = Instant::now() - (WINDOW + Duration::from_secs(1));
        }
        r.sweep_expired();
        assert_eq!(r.len(), 0, "expired entry should be pruned");
    }
}
