use lets_chat::db::chat::{is_follow_up_of, MESSAGE_GROUPING_WINDOW_SECONDS};

#[test]
fn follow_up_when_same_user_within_window() {
    assert!(is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("alice", "2026-05-04 12:00:30"),
    ));
}

#[test]
fn not_follow_up_when_different_user() {
    assert!(!is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("bob", "2026-05-04 12:00:30"),
    ));
}

#[test]
fn not_follow_up_when_gap_exceeds_window() {
    assert!(!is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("alice", "2026-05-04 12:06:00"),
    ));
}

#[test]
fn follow_up_at_exact_window_boundary() {
    assert!(is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("alice", "2026-05-04 12:05:00"),
    ));
}

#[test]
fn not_follow_up_when_no_prior() {
    assert!(!is_follow_up_of(None, ("alice", "2026-05-04 12:00:00"),));
}

#[test]
fn window_is_five_minutes() {
    assert_eq!(MESSAGE_GROUPING_WINDOW_SECONDS, 300);
}
