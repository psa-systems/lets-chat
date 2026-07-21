//! LC-592: cost and load control for server-side speech-to-text.
//!
//! Enabling STT (`stt.rs`) sends every voice message, every video clip, AND all
//! call audio to the operator's endpoint. On a metered engine that is a bill; on
//! a self-hosted CPU-bound one it is a thundering herd. Nothing in the pipeline
//! bounded any of it: each send spawned its own unbounded task, and call clips
//! ran inline once per 5 seconds per speaker.
//!
//! Three independent controls, because they bound different things:
//!
//! - **Concurrency** ([`permits`]) caps how many transcriptions run AT ONCE,
//!   which is what protects a CPU-bound engine from collapsing.
//! - **Rate** ([`global_per_minute`] / [`room_per_minute`]) caps how many are
//!   submitted PER MINUTE, which is what bounds the bill on a metered engine.
//!   Concurrency alone does not: two workers can still bill all day.
//! - **Scope** ([`scope`]) excludes whole categories, so an operator can keep
//!   cheap voice notes and drop the expensive video-clip path entirely.
//!
//! Everything here reads the environment on each call rather than caching it.
//! These are consulted a handful of times a minute at most (once per voice
//! message, once per call clip), so a `getenv` + parse is free next to the
//! network round-trip that follows, and it keeps the values honest in tests
//! instead of freezing whichever test ran first into a `OnceLock`. The one
//! exception is the semaphore itself, which MUST be a single process-wide
//! instance to bound anything at all.

use std::sync::OnceLock;

use tokio::sync::Semaphore;

/// Concurrent transcriptions allowed when `LETS_CHAT_STT_WORKERS` is unset. Two
/// is deliberately small: the common self-hosted deployment is whisper.cpp on
/// the same box as lets-chat, where a third concurrent transcription costs the
/// web server its CPU.
pub const DEFAULT_STT_WORKERS: usize = 2;

/// Server-wide submissions per minute when `LETS_CHAT_STT_RATE_GLOBAL` is unset.
pub const DEFAULT_GLOBAL_PER_MINUTE: u32 = 30;

/// Per-room submissions per minute when `LETS_CHAT_STT_RATE_ROOM` is unset. Well
/// under the global cap so one busy room cannot starve every other room.
pub const DEFAULT_ROOM_PER_MINUTE: u32 = 10;

/// LC-592: which categories of audio the operator wants transcribed. Video
/// clips are the expensive path (minutes of audio per send, versus seconds for
/// a voice note), so they get their own switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SttScope {
    /// Voice messages and video clips. The default, and the pre-LC-592
    /// behaviour.
    #[default]
    Both,
    /// Voice notes only; video clips are skipped.
    Voice,
    /// Video clips only; voice notes are skipped.
    Clips,
    /// Neither. Call captions still work - this gates stored attachments only,
    /// so an operator can keep live transcription and drop the batch load.
    None,
}

impl SttScope {
    fn parse_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "both" => Some(Self::Both),
            "voice" => Some(Self::Voice),
            "clips" => Some(Self::Clips),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn allows_voice(self) -> bool {
        matches!(self, Self::Both | Self::Voice)
    }

    pub fn allows_clips(self) -> bool {
        matches!(self, Self::Both | Self::Clips)
    }
}

/// Read a positive integer from the environment, falling back to `default` for
/// an unset, unparseable, or zero value. Zero always falls back rather than
/// meaning "disable": a zero worker count would deadlock every transcription and
/// a zero rate cap would silently switch STT off, neither of which is a
/// plausible reading of the operator's intent. Turning STT off is
/// `LETS_CHAT_STT_SCOPE=none` or unsetting `LETS_CHAT_STT_URL`.
fn env_positive<T>(key: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + From<u8>,
{
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<T>().ok())
        .filter(|v| *v > T::from(0u8))
        .unwrap_or(default)
}

/// The operator's `LETS_CHAT_STT_SCOPE`. An unrecognized value falls back to
/// `both` (the pre-LC-592 behaviour) rather than failing startup, matching how
/// `LETS_CHAT_STT_PROVIDER` treats a typo.
pub fn scope() -> SttScope {
    std::env::var("LETS_CHAT_STT_SCOPE")
        .ok()
        .and_then(|v| SttScope::parse_str(&v))
        .unwrap_or_default()
}

pub fn global_per_minute() -> u32 {
    env_positive("LETS_CHAT_STT_RATE_GLOBAL", DEFAULT_GLOBAL_PER_MINUTE)
}

pub fn room_per_minute() -> u32 {
    env_positive("LETS_CHAT_STT_RATE_ROOM", DEFAULT_ROOM_PER_MINUTE)
}

/// The process-wide concurrency limiter: `LETS_CHAT_STT_WORKERS` permits
/// (default [`DEFAULT_STT_WORKERS`]).
///
/// This is a `OnceLock` rather than a per-request value because a semaphore only
/// bounds anything if every caller shares one. It is read once, on first
/// transcription, so a test that sets the variable before touching STT still
/// gets the value it asked for.
///
/// Deliberately NOT stored on `AppState`: a permit count carries no per-instance
/// state, and threading it through would mean editing the ~117 sites that build
/// an `AppState` by hand for a value identical at every one of them.
pub fn permits() -> &'static Semaphore {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS
        .get_or_init(|| Semaphore::new(env_positive("LETS_CHAT_STT_WORKERS", DEFAULT_STT_WORKERS)))
}

/// LC-592: spend one submission against both the server-wide and the per-room
/// rate cap. `true` means the caller may transcribe.
///
/// Both counters are checked, and the global one FIRST, so that a server already
/// at its ceiling does not also burn the room's smaller allowance on a
/// submission it is going to refuse anyway. The fixed-window counters live in
/// the existing [`crate::rate_limit::RateLimits`] on `AppState`, so this needed
/// no new shared state.
pub fn try_admit(limits: &crate::rate_limit::RateLimits, room_id: i64) -> bool {
    use crate::rate_limit::{Outcome, RateLimitKind};
    if limits.check(RateLimitKind::SttGlobal, "global", global_per_minute()) != Outcome::Allow {
        return false;
    }
    limits.check(
        RateLimitKind::SttRoom,
        &room_id.to_string(),
        room_per_minute(),
    ) == Outcome::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests mutate process-global environment variables, which the
    /// harness would otherwise interleave across threads.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn scope_parsing_and_predicates() {
        assert_eq!(SttScope::parse_str("  BOTH "), Some(SttScope::Both));
        assert_eq!(SttScope::parse_str("voice"), Some(SttScope::Voice));
        assert_eq!(SttScope::parse_str("clips"), Some(SttScope::Clips));
        assert_eq!(SttScope::parse_str("none"), Some(SttScope::None));
        assert_eq!(SttScope::parse_str("sometimes"), None, "unknown -> None");

        assert!(SttScope::Both.allows_voice() && SttScope::Both.allows_clips());
        assert!(SttScope::Voice.allows_voice() && !SttScope::Voice.allows_clips());
        assert!(!SttScope::Clips.allows_voice() && SttScope::Clips.allows_clips());
        assert!(!SttScope::None.allows_voice() && !SttScope::None.allows_clips());
    }

    #[test]
    fn scope_from_env_defaults_to_both() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded under ENV_LOCK; vars removed at the end.
        unsafe { std::env::remove_var("LETS_CHAT_STT_SCOPE") };
        assert_eq!(scope(), SttScope::Both, "unset -> pre-LC-592 behaviour");
        unsafe { std::env::set_var("LETS_CHAT_STT_SCOPE", "clips") };
        assert_eq!(scope(), SttScope::Clips);
        // A typo must not silently disable transcription.
        unsafe { std::env::set_var("LETS_CHAT_STT_SCOPE", "viodeo") };
        assert_eq!(scope(), SttScope::Both);
        unsafe { std::env::remove_var("LETS_CHAT_STT_SCOPE") };
    }

    #[test]
    fn rate_caps_fall_back_on_garbage_and_zero() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded under ENV_LOCK; vars removed at the end.
        unsafe { std::env::remove_var("LETS_CHAT_STT_RATE_GLOBAL") };
        assert_eq!(global_per_minute(), DEFAULT_GLOBAL_PER_MINUTE);
        unsafe { std::env::set_var("LETS_CHAT_STT_RATE_GLOBAL", " 100 ") };
        assert_eq!(global_per_minute(), 100, "value is trimmed");
        unsafe { std::env::set_var("LETS_CHAT_STT_RATE_GLOBAL", "lots") };
        assert_eq!(global_per_minute(), DEFAULT_GLOBAL_PER_MINUTE);
        // 0 would silently switch STT off; that is what SCOPE=none is for.
        unsafe { std::env::set_var("LETS_CHAT_STT_RATE_GLOBAL", "0") };
        assert_eq!(global_per_minute(), DEFAULT_GLOBAL_PER_MINUTE);
        unsafe { std::env::remove_var("LETS_CHAT_STT_RATE_GLOBAL") };

        assert_eq!(room_per_minute(), DEFAULT_ROOM_PER_MINUTE);
        // One room must not be able to exhaust the server-wide cap on its own.
        assert!(
            room_per_minute() < global_per_minute(),
            "per-room cap {} must sit under the global cap {}",
            room_per_minute(),
            global_per_minute()
        );
    }

    #[tokio::test]
    async fn permits_bound_concurrency_and_are_shared() {
        // The same semaphore every time, or it bounds nothing.
        assert!(std::ptr::eq(permits(), permits()));
        let available = permits().available_permits();
        assert!(available >= 1, "at least one transcription can proceed");
        let held = permits().acquire().await.unwrap();
        assert_eq!(
            permits().available_permits(),
            available - 1,
            "holding a permit takes one out of circulation"
        );
        drop(held);
        assert_eq!(permits().available_permits(), available);
    }
}
