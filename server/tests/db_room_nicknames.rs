//! LC-321: per-room nickname storage.
use lets_chat::db::room_nicknames as nick;
use sqlx::SqlitePool;

mod common;

async fn setup_pool() -> SqlitePool {
    common::chat_pool().await
}

// Room ids are arbitrary integers here; the nickname table has no FK enforced
// against rooms in these unit tests (the migration FK is a no-op without a
// matching rooms row under default pragma, and we test storage semantics only).
async fn seed_room(pool: &SqlitePool, name: &str) -> i64 {
    lets_chat::db::chat::create_room(pool, name, None, "public", None, None)
        .await
        .unwrap()
}

#[tokio::test]
async fn get_is_none_when_unset() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    assert!(nick::get(&pool, room, "u1").await.unwrap().is_none());
}

#[tokio::test]
async fn set_then_get_roundtrips() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    nick::set(&pool, room, "u1", "Captain").await.unwrap();
    assert_eq!(
        nick::get(&pool, room, "u1").await.unwrap().as_deref(),
        Some("Captain")
    );
}

#[tokio::test]
async fn set_upserts_replacing_prior_value() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    nick::set(&pool, room, "u1", "Captain").await.unwrap();
    nick::set(&pool, room, "u1", "Skipper").await.unwrap();
    assert_eq!(
        nick::get(&pool, room, "u1").await.unwrap().as_deref(),
        Some("Skipper")
    );
}

#[tokio::test]
async fn clear_removes_the_nickname() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    nick::set(&pool, room, "u1", "Captain").await.unwrap();
    nick::clear(&pool, room, "u1").await.unwrap();
    assert!(nick::get(&pool, room, "u1").await.unwrap().is_none());
}

#[tokio::test]
async fn clear_when_unset_is_noop() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    nick::clear(&pool, room, "ghost").await.unwrap();
    assert!(nick::get(&pool, room, "ghost").await.unwrap().is_none());
}

#[tokio::test]
async fn nickname_is_scoped_per_room() {
    let pool = setup_pool().await;
    let r1 = seed_room(&pool, "alpha").await;
    let r2 = seed_room(&pool, "beta").await;
    nick::set(&pool, r1, "u1", "AlphaName").await.unwrap();
    assert_eq!(
        nick::get(&pool, r1, "u1").await.unwrap().as_deref(),
        Some("AlphaName")
    );
    // Same user, different room: no nickname leaks across rooms.
    assert!(nick::get(&pool, r2, "u1").await.unwrap().is_none());
}

#[tokio::test]
async fn set_rejects_overlong_nickname() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    let too_long = "x".repeat(nick::MAX_ROOM_NICKNAME_CHARS + 1);
    let err = nick::set(&pool, room, "u1", &too_long).await.unwrap_err();
    assert!(
        matches!(err, nick::SetNicknameError::TooLong(n) if n == nick::MAX_ROOM_NICKNAME_CHARS)
    );
    // Nothing persisted on rejection.
    assert!(nick::get(&pool, room, "u1").await.unwrap().is_none());
}

#[tokio::test]
async fn set_accepts_exactly_max_chars() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    let exact = "y".repeat(nick::MAX_ROOM_NICKNAME_CHARS);
    nick::set(&pool, room, "u1", &exact).await.unwrap();
    assert_eq!(
        nick::get(&pool, room, "u1").await.unwrap().map(|s| s.len()),
        Some(nick::MAX_ROOM_NICKNAME_CHARS)
    );
}

#[tokio::test]
async fn for_room_returns_all_set_nicknames() {
    let pool = setup_pool().await;
    let room = seed_room(&pool, "general").await;
    nick::set(&pool, room, "u1", "One").await.unwrap();
    nick::set(&pool, room, "u2", "Two").await.unwrap();
    let map = nick::for_room(&pool, room).await.unwrap();
    assert_eq!(map.get("u1").map(String::as_str), Some("One"));
    assert_eq!(map.get("u2").map(String::as_str), Some("Two"));
    assert_eq!(map.len(), 2);
}
