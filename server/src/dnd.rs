//! LC-88: per-user Do Not Disturb (quiet hours + manual pause).
//!
//! Two independent inputs decide whether out-of-app notifications are
//! suppressed for a user at a given instant:
//!
//! 1. A manual pause (`dnd_paused_until`): an explicit "mute everything until
//!    T" instant. While `now < T` the user is suppressed regardless of any
//!    schedule. It auto-expires by being in the past, so no sweeper is needed.
//! 2. A recurring schedule (`dnd_schedule_json`): quiet-hour windows expressed
//!    in the user's own IANA timezone, with separate weekday and weekend
//!    windows.
//!
//! Suppression is the OR of the two. The logic here is pure and timezone-aware
//! so it can be unit-tested without a database or a clock. Callers pass the
//! current UTC instant explicitly.
//!
//! Suppression gates *out-of-app delivery only* (Web Push drops, the email
//! digest holds). In-app activity records (LC-82) are written regardless; DND
//! never hides history.

use chrono::{DateTime, Datelike, NaiveTime, TimeZone, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};

use crate::models::user::UserRecord;

/// A single quiet-hours window, `start`/`end` as `"HH:MM"` strings in the
/// schedule's timezone. `start > end` means the window spans midnight
/// (e.g. `22:00`->`07:00`). `start == end` is treated as "no window" rather
/// than "all day" to avoid an accidental 24h blackout from a degenerate save.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Window {
    pub start: String,
    pub end: String,
}

/// Recurring DND schedule. Either group may be absent to leave that day-type
/// unsuppressed. An unknown/invalid timezone disables the schedule entirely
/// (fail-open: better to over-notify than to silently swallow forever).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Schedule {
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekday: Option<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekend: Option<Window>,
}

impl Schedule {
    /// Parse from the stored JSON column. Returns `None` for null/empty/invalid
    /// JSON so callers can treat "no usable schedule" uniformly.
    pub fn parse(json: Option<&str>) -> Option<Schedule> {
        let raw = json?.trim();
        if raw.is_empty() {
            return None;
        }
        serde_json::from_str::<Schedule>(raw).ok()
    }
}

/// Parse `"HH:MM"` into minutes-since-midnight. Returns `None` for malformed
/// input so a bad field disables its window instead of panicking.
fn parse_hhmm(s: &str) -> Option<u32> {
    let t = NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()?;
    Some(t.hour() * 60 + t.minute())
}

/// Is `minute_of_day` inside `window`? Handles the midnight-spanning case.
fn window_contains(window: &Window, minute_of_day: u32) -> bool {
    let (Some(start), Some(end)) = (parse_hhmm(&window.start), parse_hhmm(&window.end)) else {
        return false;
    };
    if start == end {
        return false; // degenerate: treat as no window, not all-day.
    }
    if start < end {
        minute_of_day >= start && minute_of_day < end
    } else {
        // Overnight wrap: suppressed from `start` to midnight, then to `end`.
        minute_of_day >= start || minute_of_day < end
    }
}

/// Does `schedule` suppress notifications at the UTC instant `now`?
///
/// Converts `now` into the schedule's timezone, picks the weekday or weekend
/// window for that local day, and checks the local time-of-day against it.
pub fn schedule_suppresses(schedule: &Schedule, now: DateTime<Utc>) -> bool {
    let Ok(tz) = schedule.timezone.parse::<chrono_tz::Tz>() else {
        return false; // unknown timezone: fail open.
    };
    let local = now.with_timezone(&tz);
    let window = match local.weekday() {
        Weekday::Sat | Weekday::Sun => schedule.weekend.as_ref(),
        _ => schedule.weekday.as_ref(),
    };
    let Some(window) = window else {
        return false;
    };
    let minute_of_day = local.hour() * 60 + local.minute();
    window_contains(window, minute_of_day)
}

/// Is the manual pause `dnd_paused_until` still in effect at `now`?
fn pause_active(paused_until: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(raw) = paused_until else {
        return false;
    };
    match parse_db_instant(raw) {
        Some(until) => now < until,
        None => false,
    }
}

/// Parse a stored UTC instant. Accepts both RFC3339 (`...Z`) and the bare
/// SQLite `datetime('now')` shape (`YYYY-MM-DD HH:MM:SS`, implicitly UTC).
fn parse_db_instant(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    let naive = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(Utc.from_utc_datetime(&naive))
}

/// The single decision point: are out-of-app notifications suppressed for this
/// user at `now`? Manual pause wins; otherwise the schedule decides.
pub fn is_suppressed(record: &UserRecord, now: DateTime<Utc>) -> bool {
    if pause_active(record.dnd_paused_until.as_deref(), now) {
        return true;
    }
    match Schedule::parse(record.dnd_schedule_json.as_deref()) {
        Some(schedule) => schedule_suppresses(&schedule, now),
        None => false,
    }
}

/// Lightweight variant for callers that hold only the two raw columns (e.g.
/// the digest candidate projection) rather than a full `UserRecord`.
pub fn is_suppressed_raw(
    schedule_json: Option<&str>,
    paused_until: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    if pause_active(paused_until, now) {
        return true;
    }
    match Schedule::parse(schedule_json) {
        Some(schedule) => schedule_suppresses(&schedule, now),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn sched(json: &str) -> Schedule {
        Schedule::parse(Some(json)).unwrap()
    }

    #[test]
    fn no_schedule_no_pause_never_suppresses() {
        assert!(!is_suppressed_raw(None, None, at("2026-05-22T03:00:00Z")));
    }

    #[test]
    fn overnight_weekday_window_in_local_tz() {
        // 22:00->07:00 New York. 2026-05-22 is a Friday (weekday).
        let s =
            sched(r#"{"timezone":"America/New_York","weekday":{"start":"22:00","end":"07:00"}}"#);
        // 05:00 UTC == 01:00 EDT -> inside the overnight window.
        assert!(schedule_suppresses(&s, at("2026-05-22T05:00:00Z")));
        // 18:00 UTC == 14:00 EDT -> outside.
        assert!(!schedule_suppresses(&s, at("2026-05-22T18:00:00Z")));
    }

    #[test]
    fn weekend_window_distinct_from_weekday() {
        // Weekday group only; on a Saturday it must NOT suppress.
        let s = sched(r#"{"timezone":"UTC","weekday":{"start":"09:00","end":"17:00"}}"#);
        // 2026-05-23 is a Saturday.
        assert!(!schedule_suppresses(&s, at("2026-05-23T12:00:00Z")));
        // 2026-05-22 Friday noon -> suppressed.
        assert!(schedule_suppresses(&s, at("2026-05-22T12:00:00Z")));
    }

    #[test]
    fn weekend_group_applies_on_saturday_and_sunday() {
        let s = sched(r#"{"timezone":"UTC","weekend":{"start":"00:00","end":"23:59"}}"#);
        assert!(schedule_suppresses(&s, at("2026-05-23T12:00:00Z"))); // Sat
        assert!(schedule_suppresses(&s, at("2026-05-24T12:00:00Z"))); // Sun
        assert!(!schedule_suppresses(&s, at("2026-05-22T12:00:00Z"))); // Fri
    }

    #[test]
    fn daytime_window_no_wrap() {
        let s = sched(r#"{"timezone":"UTC","weekday":{"start":"09:00","end":"17:00"}}"#);
        assert!(schedule_suppresses(&s, at("2026-05-22T09:00:00Z"))); // start inclusive
        assert!(!schedule_suppresses(&s, at("2026-05-22T17:00:00Z"))); // end exclusive
        assert!(!schedule_suppresses(&s, at("2026-05-22T08:59:00Z")));
    }

    #[test]
    fn degenerate_equal_window_is_disabled() {
        let s = sched(r#"{"timezone":"UTC","weekday":{"start":"09:00","end":"09:00"}}"#);
        assert!(!schedule_suppresses(&s, at("2026-05-22T09:00:00Z")));
        assert!(!schedule_suppresses(&s, at("2026-05-22T15:00:00Z")));
    }

    #[test]
    fn unknown_timezone_fails_open() {
        let s = sched(r#"{"timezone":"Not/AZone","weekday":{"start":"00:00","end":"23:59"}}"#);
        assert!(!schedule_suppresses(&s, at("2026-05-22T12:00:00Z")));
    }

    #[test]
    fn manual_pause_supersedes_and_expires() {
        let until = "2026-05-22T10:00:00Z";
        assert!(is_suppressed_raw(
            None,
            Some(until),
            at("2026-05-22T09:30:00Z")
        ));
        assert!(!is_suppressed_raw(
            None,
            Some(until),
            at("2026-05-22T10:30:00Z")
        ));
    }

    #[test]
    fn pause_accepts_bare_sqlite_instant() {
        // datetime('now') shape, implicitly UTC.
        assert!(pause_active(
            Some("2026-05-22 10:00:00"),
            at("2026-05-22T09:00:00Z")
        ));
        assert!(!pause_active(
            Some("2026-05-22 10:00:00"),
            at("2026-05-22T11:00:00Z")
        ));
    }

    #[test]
    fn invalid_schedule_json_is_ignored() {
        assert!(Schedule::parse(Some("not json")).is_none());
        assert!(Schedule::parse(Some("")).is_none());
        assert!(Schedule::parse(None).is_none());
    }
}
