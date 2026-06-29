//! LC-495: persistence tests for room workflow-automation rules.

mod common;

use lets_chat::db;

#[tokio::test]
async fn insert_list_and_active_filter() {
    let pool = common::chat_pool().await;

    let id1 = db::automations::insert(
        &pool,
        1,
        Some("Welcome"),
        "message_posted",
        Some("help"),
        "post_message",
        "Need a hand? Ask in #support.",
        "u-admin",
    )
    .await
    .expect("insert 1");

    // A reaction rule with no filter (fires on any emoji).
    let _id2 = db::automations::insert(
        &pool,
        1,
        None,
        "reaction_added",
        None,
        "post_message",
        "Thanks for the reaction!",
        "u-admin",
    )
    .await
    .expect("insert 2");

    // A rule in a different room must not leak into room 1's queries.
    db::automations::insert(
        &pool,
        2,
        None,
        "message_posted",
        None,
        "post_message",
        "other room",
        "u-admin",
    )
    .await
    .expect("insert other room");

    let all = db::automations::list_for_room(&pool, 1).await.unwrap();
    assert_eq!(all.len(), 2, "room 1 has two rules");
    assert_eq!(db::automations::count_for_room(&pool, 1).await.unwrap(), 2);

    let msg_rules = db::automations::list_active_for_trigger(&pool, 1, "message_posted")
        .await
        .unwrap();
    assert_eq!(msg_rules.len(), 1);
    assert_eq!(msg_rules[0].id, id1);
    assert_eq!(msg_rules[0].match_text.as_deref(), Some("help"));

    let react_rules = db::automations::list_active_for_trigger(&pool, 1, "reaction_added")
        .await
        .unwrap();
    assert_eq!(react_rules.len(), 1);
    assert!(react_rules[0].match_text.is_none());
}

#[tokio::test]
async fn toggle_excludes_from_active_and_is_room_scoped() {
    let pool = common::chat_pool().await;
    let id = db::automations::insert(
        &pool,
        5,
        None,
        "message_posted",
        Some("ping"),
        "post_message",
        "pong",
        "u-admin",
    )
    .await
    .unwrap();

    // Disabling drops it from the active query but keeps it in the full list.
    let n = db::automations::set_enabled(&pool, id, 5, false)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(
        db::automations::list_active_for_trigger(&pool, 5, "message_posted")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db::automations::list_for_room(&pool, 5)
            .await
            .unwrap()
            .len(),
        1
    );

    // A toggle scoped to the WRONG room is a no-op (forged id defense).
    let n = db::automations::set_enabled(&pool, id, 999, true)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(
        !db::automations::get(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );

    // Correct room re-enables it.
    let n = db::automations::set_enabled(&pool, id, 5, true)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(
        db::automations::get(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn delete_is_room_scoped() {
    let pool = common::chat_pool().await;
    let id = db::automations::insert(
        &pool,
        7,
        None,
        "reaction_added",
        Some("🎉"),
        "post_message",
        "party",
        "u-admin",
    )
    .await
    .unwrap();

    // Wrong room: no delete.
    assert_eq!(db::automations::delete(&pool, id, 8).await.unwrap(), 0);
    assert!(db::automations::get(&pool, id).await.unwrap().is_some());

    // Right room: gone.
    assert_eq!(db::automations::delete(&pool, id, 7).await.unwrap(), 1);
    assert!(db::automations::get(&pool, id).await.unwrap().is_none());
}
