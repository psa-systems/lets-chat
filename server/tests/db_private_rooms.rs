mod common;

async fn setup_pool() -> sqlx::SqlitePool {
    common::chat_pool().await
}

/// LC-606: a private channel inside an enclave `user_id` belongs to.
///
/// `list_rooms` is now filtered by the shared visibility predicate, which
/// mirrors `is_room_accessible`: enclave membership is checked *before* room
/// membership, and a channel with a NULL `enclave_id` is unreachable for a
/// non-admin. A room created with `enclave_id = None` therefore cannot be
/// listed, no matter who joined it - and no production channel looks like that,
/// since migration 0009 backfills every non-DM room into an enclave. Previously
/// `list_rooms` would happily show such a room while opening it returned 403.
async fn private_room_in_enclave(pool: &sqlx::SqlitePool, user_id: &str) -> i64 {
    let enclave_id = lets_chat::db::enclave::create_enclave(pool, "Test Enclave", None, user_id)
        .await
        .unwrap();
    lets_chat::db::enclave::add_member(
        pool,
        enclave_id,
        user_id,
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    lets_chat::db::chat::create_room(
        pool,
        "secret",
        None,
        "private",
        Some("invite-abc"),
        Some(enclave_id),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn test_list_rooms_excludes_private_for_non_member() {
    let pool = setup_pool().await;

    // LC-606: the viewer is a member of the *enclave* but not of the private
    // room. Without the enclave membership this assertion would hold trivially
    // (nothing is visible), so it would pass while testing nothing.
    let room_id = private_room_in_enclave(&pool, "owner-1").await;
    lets_chat::db::chat::add_room_member(&pool, room_id, "owner-1")
        .await
        .unwrap();
    // The helper makes exactly one enclave, owned by owner-1; add user-1 to it.
    let enclave_id = lets_chat::db::enclave::list_enclaves_for_user(&pool, "owner-1")
        .await
        .unwrap()
        .first()
        .expect("owner is in an enclave")
        .id;
    lets_chat::db::enclave::add_member(
        &pool,
        enclave_id,
        "user-1",
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();

    let rooms = lets_chat::db::chat::list_rooms(&pool, "user-1", false)
        .await
        .unwrap();
    assert!(
        rooms.iter().all(|r| r.room_type != "private"),
        "an enclave member who has not joined the private room must not see it"
    );

    // ...and the room member does, so the assertion above is about room
    // membership rather than about everything being filtered out.
    let owner_rooms = lets_chat::db::chat::list_rooms(&pool, "owner-1", false)
        .await
        .unwrap();
    assert!(
        owner_rooms.iter().any(|r| r.name == "secret"),
        "the room member should see it"
    );
}

#[tokio::test]
async fn test_list_rooms_includes_private_for_member() {
    let pool = setup_pool().await;

    let room_id = private_room_in_enclave(&pool, "user-1").await;
    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();

    // Member should see the private room
    let rooms = lets_chat::db::chat::list_rooms(&pool, "user-1", false)
        .await
        .unwrap();
    assert!(
        rooms.iter().any(|r| r.name == "secret"),
        "member should see private room they joined"
    );
}

#[tokio::test]
async fn test_admin_sees_all_rooms_including_private() {
    let pool = setup_pool().await;

    lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("invite-abc"), None)
        .await
        .unwrap();

    // Admin sees all - not a member but is_admin = true
    let rooms = lets_chat::db::chat::list_rooms(&pool, "admin-user", true)
        .await
        .unwrap();
    assert!(
        rooms.iter().any(|r| r.name == "secret"),
        "admin should see private rooms regardless of membership"
    );
}

#[tokio::test]
async fn test_get_room_by_invite_code() {
    let pool = setup_pool().await;

    lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("invite-xyz"), None)
        .await
        .unwrap();

    let found = lets_chat::db::chat::get_room_by_invite(&pool, "invite-xyz")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "secret");

    let not_found = lets_chat::db::chat::get_room_by_invite(&pool, "wrong-code")
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_is_room_member() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("code"), None)
            .await
            .unwrap();

    assert!(
        !lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );

    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();
    assert!(
        lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );

    lets_chat::db::chat::remove_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();
    assert!(
        !lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_add_room_member_is_idempotent() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("code"), None)
            .await
            .unwrap();

    // Adding same user twice should not error (INSERT OR IGNORE)
    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();
    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();

    assert!(
        lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_regenerate_invite_code() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("old-code"), None)
            .await
            .unwrap();

    lets_chat::db::chat::regenerate_invite_code(&pool, room_id, "new-code")
        .await
        .unwrap();

    // Old code should no longer work
    let by_old = lets_chat::db::chat::get_room_by_invite(&pool, "old-code")
        .await
        .unwrap();
    assert!(by_old.is_none());

    // New code should work
    let by_new = lets_chat::db::chat::get_room_by_invite(&pool, "new-code")
        .await
        .unwrap();
    assert!(by_new.is_some());
    assert_eq!(by_new.unwrap().id, room_id);
}
