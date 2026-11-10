//! LC-1148: demoting a site admin must drop their live connections from any
//! room subscription that only existed because of the god-mode grant in
//! `is_room_accessible`, while leaving a genuine member's subscription to the
//! same room untouched.
use lets_chat::db;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;

use crate::common;

#[tokio::test]
async fn demotion_drops_the_non_member_admins_room_subscription_but_not_a_members() {
    let chat = common::chat_pool().await;

    let member = "room-member".to_string();
    let admin = "god-mode-admin".to_string();

    let enclave_id = db::enclave::create_enclave(&chat, "Team", None, &member)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None, Some(enclave_id))
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room_id, &member)
        .await
        .unwrap();

    // Sanity: the admin is not a member of the enclave or the room, so their
    // only route to the room's subscription is the site-admin god-mode grant.
    assert!(!db::enclave::is_enclave_member(&chat, enclave_id, &admin)
        .await
        .unwrap());
    assert!(
        db::chat::is_room_accessible(&chat, room_id, &admin, true)
            .await
            .unwrap(),
        "god-mode grants the admin access while still admin"
    );
    assert!(
        !db::chat::is_room_accessible(&chat, room_id, &admin, false)
            .await
            .unwrap(),
        "without the grant the admin has no legitimate access to this room"
    );

    let hub = Hub::new();
    let (member_conn, mut member_rx, _) = hub.connect(&member, "member");
    let (admin_conn, mut admin_rx, _) = hub.connect(&admin, "admin");

    // Both subscribed to the room's broadcast set: the member legitimately,
    // the admin only via the god-mode path (mirrors the WS subscribe handler
    // granting it per `topic_subscribe_allowed`/`is_room_accessible`).
    hub.subscribe(member_conn, room_id);
    hub.subscribe(admin_conn, room_id);

    let evt = ChatEvent::UserTyping {
        room_id,
        user_id: "someone-else".into(),
        username: "someone-else".into(),
    };
    hub.broadcast_to_room(room_id, &evt);
    assert!(
        member_rx.try_recv().is_ok(),
        "member receives before demotion"
    );
    assert!(
        admin_rx.try_recv().is_ok(),
        "admin receives before demotion"
    );

    // Demotion: drop every room subscription that would fail without the
    // admin grant.
    hub.unsubscribe_user_from_non_member_rooms(&admin, &chat)
        .await;

    hub.broadcast_to_room(room_id, &evt);
    assert!(
        admin_rx.try_recv().is_err(),
        "demoted non-member admin must stop receiving this room's broadcasts"
    );
    assert!(
        member_rx.try_recv().is_ok(),
        "a genuine member's subscription to the same room must be unaffected"
    );
}
