//! LC-914: `call_transcripts` "one open session per room" invariant, covered
//! at the DB layer directly (the HTTP surface is covered in `transcripts.rs`).

use sqlx::SqlitePool;

async fn chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    sqlx::migrate!("./migrations/chat")
        .run(&pool)
        .await
        .expect("chat migrations");
    pool
}

/// The common case (sequential, not racing): a second `start_session` for a
/// room with an already-open session joins it instead of forking a duplicate,
/// and reports `created = false` so the caller (routes::transcripts::start)
/// knows not to dispatch the agent again.
#[tokio::test]
async fn second_start_session_joins_and_reports_not_created() {
    let pool = chat_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "voicechan", None, "public", None, None)
        .await
        .unwrap();

    let (first, created_first) = lets_chat::db::transcripts::start_session(&pool, room_id, "alice")
        .await
        .unwrap();
    assert!(created_first, "first call opens a new session");

    let (second, created_second) = lets_chat::db::transcripts::start_session(&pool, room_id, "bob")
        .await
        .unwrap();
    assert!(!created_second, "second call joins the existing session");
    assert_eq!(first.id, second.id);

    let active: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM call_transcripts WHERE room_id = ? AND status = 'active'",
    )
    .bind(room_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(active, vec![first.id]);
}

/// LC-914 AC: two concurrent `start_session` calls for the same room must
/// still yield exactly one `call_transcripts` row, both resolving to the same
/// transcript id, with `created = true` for exactly one of the two calls -
/// that flag is what `routes::transcripts::start` uses (instead of a separate
/// read) to decide whether to dispatch the transcription agent, so this is
/// also the evidence that the dispatch path is entered exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_start_session_calls_create_exactly_one_session() {
    let pool = chat_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "voicechan", None, "public", None, None)
        .await
        .unwrap();

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let (res_a, res_b) = tokio::join!(
        tokio::spawn(async move {
            lets_chat::db::transcripts::start_session(&pool_a, room_id, "alice").await
        }),
        tokio::spawn(async move {
            lets_chat::db::transcripts::start_session(&pool_b, room_id, "bob").await
        }),
    );
    let (session_a, created_a) = res_a.expect("join a").expect("call a");
    let (session_b, created_b) = res_b.expect("join b").expect("call b");

    assert_eq!(
        session_a.id, session_b.id,
        "both concurrent calls must resolve to the same transcript id"
    );
    assert_eq!(
        [created_a, created_b].iter().filter(|c| **c).count(),
        1,
        "exactly one of the two racing calls must report having created the session"
    );

    let active: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM call_transcripts WHERE room_id = ? AND status = 'active'",
    )
    .bind(room_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        active,
        vec![session_a.id],
        "exactly one active row for the room"
    );
}

/// LC-914: the 0099 migration's dedupe step (run before the unique index is
/// created) must resolve pre-existing duplicate active rows deterministically
/// by ending every one except the highest-id (most recently opened) row.
/// Reproduced here against a hand-built duplicate state, since a fresh pool
/// already has the unique index applied and cannot hold duplicates directly.
#[tokio::test]
async fn dedupe_step_keeps_only_the_highest_id_active_row_per_room() {
    let pool = chat_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "voicechan", None, "public", None, None)
        .await
        .unwrap();
    let other_room = lets_chat::db::chat::create_room(&pool, "other", None, "public", None, None)
        .await
        .unwrap();

    sqlx::query("DROP INDEX idx_call_transcripts_one_open")
        .execute(&pool)
        .await
        .unwrap();
    for started_by in ["alice", "bob", "carol"] {
        sqlx::query(
            "INSERT INTO call_transcripts (room_id, started_by, status) VALUES (?, ?, 'active')",
        )
        .bind(room_id)
        .bind(started_by)
        .execute(&pool)
        .await
        .unwrap();
    }
    // An unrelated room's own active session must be untouched by the dedupe.
    sqlx::query(
        "INSERT INTO call_transcripts (room_id, started_by, status) VALUES (?, 'dave', 'active')",
    )
    .bind(other_room)
    .execute(&pool)
    .await
    .unwrap();

    // The exact statement from 0099_transcript_one_open_session.sql.
    sqlx::query(
        "UPDATE call_transcripts \
         SET status = 'ended', ended_at = datetime('now') \
         WHERE status = 'active' \
           AND id NOT IN ( \
               SELECT MAX(id) FROM call_transcripts WHERE status = 'active' GROUP BY room_id \
           )",
    )
    .execute(&pool)
    .await
    .unwrap();

    let active_in_room: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, started_by FROM call_transcripts WHERE room_id = ? AND status = 'active'",
    )
    .bind(room_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        active_in_room,
        vec![(3, "carol".to_string())],
        "only the highest-id row for the room stays active"
    );

    let active_in_other: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM call_transcripts WHERE room_id = ? AND status = 'active'",
    )
    .bind(other_room)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        active_in_other, 1,
        "the other room's own session is untouched"
    );
}
