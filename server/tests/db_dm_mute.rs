use lets_chat::db::notifications::{
    self, room_mute_mode, set_dm_mute, set_room_mute_mode, MuteMode,
};
use sqlx::SqlitePool;
mod common;

async fn setup_chat_pool() -> sqlx::SqlitePool {
    common::chat_pool().await
}

async fn seed_room(pool: &SqlitePool, name: &str, kind: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, ?)")
        .bind(name)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn set_dm_mute_writes_all_mode() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
}

#[tokio::test]
async fn set_dm_mute_false_deletes_row() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_dm_mute(&pool, "user-1", dm, false).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind("user-1")
    .bind(dm)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn set_dm_mute_idempotent_when_already_muted() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
}

#[tokio::test]
async fn set_dm_mute_rejects_non_dm_room() {
    let pool = setup_chat_pool().await;
    let public = seed_room(&pool, "general", "public").await;
    let res = set_dm_mute(&pool, "user-1", public, true).await;
    assert!(res.is_err(), "expected error, got {res:?}");
}

#[tokio::test]
async fn set_dm_mute_rejects_missing_room() {
    let pool = setup_chat_pool().await;
    let res = set_dm_mute(&pool, "user-1", 9_999, true).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn set_room_mute_mode_rejects_dm_room() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    let res = set_room_mute_mode(&pool, "user-1", dm, MuteMode::All).await;
    assert!(res.is_err(), "expected DM rejection, got {res:?}");
    let res = set_room_mute_mode(&pool, "user-1", dm, MuteMode::ExceptMentions).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn set_room_mute_mode_rejects_missing_room() {
    let pool = setup_chat_pool().await;
    let res = set_room_mute_mode(&pool, "user-1", 9_999, MuteMode::All).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn dm_mute_per_direction_independent() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-a", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-a", dm).await.unwrap(),
        MuteMode::All
    );
    assert_eq!(
        room_mute_mode(&pool, "user-b", dm).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn dm_mute_does_not_collide_with_room_mute_for_other_room() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    let r = seed_room(&pool, "general", "public").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
    let _ = notifications::room_mute_modes_for_user(&pool, "user-1")
        .await
        .unwrap();
}
