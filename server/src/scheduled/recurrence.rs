//! LC-485: recurrence math for recurring scheduled messages.
//!
//! Given a fired occurrence's `scheduled_for` (UTC, `%Y-%m-%d %H:%M:%S`) and a
//! `repeat` kind, compute the next occurrence's timestamp in the same format
//! the rest of the scheduled pipeline stores + compares against
//! `datetime('now')`. The result is advanced past `now` so a dispatcher that
//! was down for a while does not re-fire a backlog of past occurrences - it
//! catches up to the next future slot.

use chrono::{Datelike, Duration, NaiveDateTime, Utc, Weekday};

const TS_FMT: &str = "%Y-%m-%d %H:%M:%S";
/// Upper bound on advance iterations - purely an infinite-loop guard. The
/// normal path is 1-2 steps (the base is near now); a large value only matters
/// if the dispatcher was down for years, and each step is a cheap date add.
/// 20000 daily steps covers ~54 years.
const MAX_ADVANCE: u32 = 20_000;

/// The valid `repeat` values (also the CHECK-constraint set).
pub fn is_valid_repeat(repeat: &str) -> bool {
    matches!(repeat, "none" | "daily" | "weekly" | "weekdays")
}

/// Compute the next occurrence after `scheduled_for` for `repeat`, strictly in
/// the future relative to now. `None` for `repeat == "none"`, an unknown kind,
/// or an unparseable timestamp (caller then simply does not re-enqueue).
pub fn next_occurrence(scheduled_for: &str, repeat: &str) -> Option<String> {
    next_occurrence_from(scheduled_for, repeat, Utc::now().naive_utc())
}

/// Testable core: `now` is injected so unit tests are deterministic.
pub fn next_occurrence_from(
    scheduled_for: &str,
    repeat: &str,
    now: NaiveDateTime,
) -> Option<String> {
    if repeat == "none" {
        return None;
    }
    let base = NaiveDateTime::parse_from_str(scheduled_for.trim(), TS_FMT).ok()?;
    let mut next = base;
    for _ in 0..MAX_ADVANCE {
        next = step(next, repeat)?;
        if next > now {
            break;
        }
    }
    Some(next.format(TS_FMT).to_string())
}

/// One step forward for `repeat`. `weekdays` skips Saturday/Sunday.
fn step(from: NaiveDateTime, repeat: &str) -> Option<NaiveDateTime> {
    match repeat {
        "daily" => Some(from + Duration::days(1)),
        "weekly" => Some(from + Duration::days(7)),
        "weekdays" => {
            let mut n = from + Duration::days(1);
            while matches!(n.weekday(), Weekday::Sat | Weekday::Sun) {
                n += Duration::days(1);
            }
            Some(n)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, TS_FMT).unwrap()
    }

    #[test]
    fn none_never_recurs() {
        assert_eq!(
            next_occurrence_from("2026-06-01 09:00:00", "none", dt("2026-06-01 09:00:00")),
            None
        );
    }

    #[test]
    fn daily_advances_one_day() {
        // Fired at 09:00, now just after; next is the following day same time.
        let next = next_occurrence_from("2026-06-01 09:00:00", "daily", dt("2026-06-01 09:00:05"));
        assert_eq!(next.as_deref(), Some("2026-06-02 09:00:00"));
    }

    #[test]
    fn weekly_advances_seven_days() {
        let next = next_occurrence_from("2026-06-01 09:00:00", "weekly", dt("2026-06-01 09:00:05"));
        assert_eq!(next.as_deref(), Some("2026-06-08 09:00:00"));
    }

    #[test]
    fn daily_catches_up_past_a_downtime_gap() {
        // Scheduled long ago, dispatcher was down: skip to the next future slot.
        let next = next_occurrence_from("2026-06-01 09:00:00", "daily", dt("2026-06-05 12:00:00"));
        assert_eq!(next.as_deref(), Some("2026-06-06 09:00:00"));
    }

    #[test]
    fn weekdays_skips_the_weekend() {
        // 2026-06-05 is a Friday -> next weekday is Monday 2026-06-08.
        let next =
            next_occurrence_from("2026-06-05 09:00:00", "weekdays", dt("2026-06-05 09:00:05"));
        assert_eq!(next.as_deref(), Some("2026-06-08 09:00:00"));
    }

    #[test]
    fn weekdays_midweek_advances_one_day() {
        // 2026-06-03 is a Wednesday -> Thursday.
        let next =
            next_occurrence_from("2026-06-03 09:00:00", "weekdays", dt("2026-06-03 09:00:05"));
        assert_eq!(next.as_deref(), Some("2026-06-04 09:00:00"));
    }
}
