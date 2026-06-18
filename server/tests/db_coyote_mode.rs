//! LC-339: Coyote Mode DB-layer invariants - the toggle, enclave ban-list,
//! cross-room burst counter, and scoped 24h soft-purge. Uses the migrate!-backed
//! common chat pool so migration 0064 is applied automatically.

mod common;

use lets_chat::db;
use lets_chat::models::enclave::EnclaveRole;
use sqlx::SqlitePool;

async fn enclave(pool: &SqlitePool, name: &str) -> i64 {
    db::enclave::create_enclave(pool, name, None, "owner")
        .await
        .unwrap()
}

async fn room(pool: &SqlitePool, name: &str, eid: i64) -> i64 {
    db::chat::create_room(pool, name, None, "public", None, Some(eid))
        .await
        .unwrap()
}

#[tokio::test]
async fn coyote_mode_toggle_round_trips() {
    let pool = common::chat_pool().await;
    let eid = enclave(&pool, "E").await;

    let e = db::enclave::get_enclave(&pool, eid).await.unwrap().unwrap();
    assert!(!e.coyote_mode, "default off");

    db::enclave::set_coyote_mode(&pool, eid, true)
        .await
        .unwrap();
    let e = db::enclave::get_enclave(&pool, eid).await.unwrap().unwrap();
    assert!(e.coyote_mode);

    db::enclave::set_coyote_mode(&pool, eid, false)
        .await
        .unwrap();
    let e = db::enclave::get_enclave(&pool, eid).await.unwrap().unwrap();
    assert!(!e.coyote_mode);
}

#[tokio::test]
async fn ban_from_enclave_kicks_and_blocks() {
    let pool = common::chat_pool().await;
    let eid = enclave(&pool, "E").await;
    db::enclave::add_member(&pool, eid, "bot", EnclaveRole::Member)
        .await
        .unwrap();

    assert!(!db::enclave::is_enclave_banned(&pool, eid, "bot")
        .await
        .unwrap());
    db::enclave::ban_from_enclave(&pool, eid, "bot", "test")
        .await
        .unwrap();

    assert!(db::enclave::is_enclave_banned(&pool, eid, "bot")
        .await
        .unwrap());
    assert!(
        db::enclave::get_membership(&pool, eid, "bot")
            .await
            .unwrap()
            .is_none(),
        "ban removes membership"
    );
    // Idempotent: a second ban does not error.
    db::enclave::ban_from_enclave(&pool, eid, "bot", "test")
        .await
        .unwrap();
    // A different user is unaffected.
    assert!(!db::enclave::is_enclave_banned(&pool, eid, "other")
        .await
        .unwrap());
}

#[tokio::test]
async fn list_and_unban_round_trip() {
    let pool = common::chat_pool().await;
    let eid = enclave(&pool, "E").await;
    db::enclave::ban_from_enclave(&pool, eid, "bot1", "coyote_mode: cross-room burst")
        .await
        .unwrap();
    db::enclave::ban_from_enclave(&pool, eid, "bot2", "manual")
        .await
        .unwrap();

    let bans = db::enclave::list_enclave_bans(&pool, eid).await.unwrap();
    assert_eq!(bans.len(), 2);
    // Bans carry their reason; both users are present.
    let users: Vec<&str> = bans.iter().map(|b| b.user_id.as_str()).collect();
    assert!(users.contains(&"bot1") && users.contains(&"bot2"));
    assert!(bans.iter().any(|b| b.reason.as_deref() == Some("manual")));

    db::enclave::unban_from_enclave(&pool, eid, "bot1")
        .await
        .unwrap();
    assert!(!db::enclave::is_enclave_banned(&pool, eid, "bot1")
        .await
        .unwrap());
    assert!(db::enclave::is_enclave_banned(&pool, eid, "bot2")
        .await
        .unwrap());
    let bans = db::enclave::list_enclave_bans(&pool, eid).await.unwrap();
    assert_eq!(bans.len(), 1);
    assert_eq!(bans[0].user_id, "bot2");

    // Unbanning a non-banned user is a harmless no-op.
    db::enclave::unban_from_enclave(&pool, eid, "ghost")
        .await
        .unwrap();
}

#[tokio::test]
async fn distinct_room_count_is_window_and_enclave_scoped() {
    let pool = common::chat_pool().await;
    let eid = enclave(&pool, "E").await;
    let other = enclave(&pool, "Other").await;
    let r1 = room(&pool, "r1", eid).await;
    let r2 = room(&pool, "r2", eid).await;
    let r3 = room(&pool, "r3", eid).await;
    let ro = room(&pool, "ro", other).await;

    // Two distinct rooms in-window -> 2.
    db::chat::insert_message(&pool, r1, "bot", "a")
        .await
        .unwrap();
    db::chat::insert_message(&pool, r2, "bot", "b")
        .await
        .unwrap();
    assert_eq!(
        db::enclave::count_distinct_rooms_posted_recently(&pool, eid, "bot", 3)
            .await
            .unwrap(),
        2
    );

    // Third distinct room -> 3 (the bot signal).
    db::chat::insert_message(&pool, r3, "bot", "c")
        .await
        .unwrap();
    assert_eq!(
        db::enclave::count_distinct_rooms_posted_recently(&pool, eid, "bot", 3)
            .await
            .unwrap(),
        3
    );

    // A post in another enclave's room is not counted for this enclave.
    db::chat::insert_message(&pool, ro, "bot", "d")
        .await
        .unwrap();
    assert_eq!(
        db::enclave::count_distinct_rooms_posted_recently(&pool, eid, "bot", 3)
            .await
            .unwrap(),
        3
    );

    // A message older than the window is excluded.
    sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, created_at) \
         VALUES (?, ?, ?, datetime('now','-30 seconds'))",
    )
    .bind(room(&pool, "old", eid).await)
    .bind("bot")
    .bind("e")
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        db::enclave::count_distinct_rooms_posted_recently(&pool, eid, "bot", 3)
            .await
            .unwrap(),
        3,
        "the 30s-old room must not count"
    );

    // Another user's bursts do not count toward this user.
    assert_eq!(
        db::enclave::count_distinct_rooms_posted_recently(&pool, eid, "ghost", 3)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn soft_purge_is_scoped_to_enclave_user_and_24h() {
    let pool = common::chat_pool().await;
    let eid = enclave(&pool, "E").await;
    let other = enclave(&pool, "Other").await;
    let r1 = room(&pool, "r1", eid).await;
    let r2 = room(&pool, "r2", eid).await;
    let ro = room(&pool, "ro", other).await;

    let m1 = db::chat::insert_message(&pool, r1, "bot", "x")
        .await
        .unwrap();
    let m2 = db::chat::insert_message(&pool, r2, "bot", "y")
        .await
        .unwrap();
    // Out of scope: other enclave, other user, and an old (>24h) message.
    let other_enclave_msg = db::chat::insert_message(&pool, ro, "bot", "z")
        .await
        .unwrap();
    let other_user_msg = db::chat::insert_message(&pool, r1, "human", "hi")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, created_at) \
         VALUES (?, ?, ?, datetime('now','-25 hours'))",
    )
    .bind(r1)
    .bind("bot")
    .bind("old")
    .execute(&pool)
    .await
    .unwrap();

    let purged =
        db::moderation::soft_delete_user_messages_in_enclave(&pool, eid, "bot", "system:coyote")
            .await
            .unwrap();
    let mut ids: Vec<i64> = purged.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![m1, m2], "only this enclave's <24h bot messages");

    let deleted = |id: i64| {
        let pool = pool.clone();
        async move {
            let row: Option<String> =
                sqlx::query_scalar("SELECT deleted_at FROM messages WHERE id=?")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            row.is_some()
        }
    };
    assert!(deleted(m1).await && deleted(m2).await);
    assert!(!deleted(other_enclave_msg).await, "other enclave untouched");
    assert!(!deleted(other_user_msg).await, "other user untouched");

    // Re-running purges nothing (already soft-deleted).
    let again =
        db::moderation::soft_delete_user_messages_in_enclave(&pool, eid, "bot", "system:coyote")
            .await
            .unwrap();
    assert!(again.is_empty());
}
