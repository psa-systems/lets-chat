//! LC-604: the unread-inbox and dashboard queries must not surface rooms the
//! viewer would be refused with a 403.
//!
//! `db::inbox::list_unread` and `db::chat::list_room_unread_counts` used to
//! filter on `room_type = 'public' OR room_members`, with no enclave condition.
//! A room is only public *within its enclave*, and `is_room_accessible` (which
//! backs `require_room_access`) requires enclave membership first - so a
//! non-member was refused the room while still being shown its name, its unread
//! count, and the body of its newest message on `/inbox` and the home dashboard.
//!
//! These tests pin the two queries to `is_room_accessible`, which is the
//! predicate that decides the 403.
use lets_chat::db;

mod common;

struct Fx {
    chat: sqlx::SqlitePool,
    /// Member of the enclave, and the viewer in the positive cases.
    insider: String,
    /// In no enclave at all.
    outsider: String,
    enclave_id: i64,
    room_id: i64,
}

/// A public room inside a private enclave, holding one message. The message is
/// authored by a *third* user because `list_unread` filters `m.user_id != ?` -
/// a viewer never sees their own message as unread, so an insider-authored
/// message would make the positive assertions vacuous.
///
/// The enclave id is returned rather than assumed: migration 0009 already seeds
/// a "General" enclave, so a freshly created one is not id 1.
async fn fixture() -> Fx {
    let chat = common::chat_pool().await;

    let insider = "insider-user".to_string();
    let outsider = "outsider-user".to_string();
    let poster = "poster-user".to_string();

    let enclave_id = db::enclave::create_enclave(&chat, "Private Team", None, &insider)
        .await
        .unwrap();
    db::enclave::add_member(
        &chat,
        enclave_id,
        &poster,
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    let room_id = db::chat::create_room(
        &chat,
        "secret-general",
        None,
        "public",
        None,
        Some(enclave_id),
    )
    .await
    .unwrap();
    db::chat::insert_message(&chat, room_id, &poster, "the secret message body")
        .await
        .unwrap();

    Fx {
        chat,
        insider,
        outsider,
        enclave_id,
        room_id,
    }
}

#[tokio::test]
async fn is_room_accessible_refuses_the_outsider() {
    // Establishes the baseline the other tests are pinned to: this is the call
    // that produces the 403, so anything the list queries show beyond it leaks.
    let fx = fixture().await;
    let (chat, insider, outsider, room_id) = (&fx.chat, &fx.insider, &fx.outsider, fx.room_id);

    assert!(
        db::chat::is_room_accessible(chat, room_id, insider, false)
            .await
            .unwrap(),
        "an enclave member may read a public room in their enclave"
    );
    assert!(
        !db::chat::is_room_accessible(chat, room_id, outsider, false)
            .await
            .unwrap(),
        "a non-member is refused a public room in an enclave they are not in"
    );
}

#[tokio::test]
async fn inbox_hides_rooms_from_enclaves_the_viewer_is_not_in() {
    let fx = fixture().await;
    let (chat, insider, outsider, room_id) = (&fx.chat, &fx.insider, &fx.outsider, fx.room_id);

    let insider_rows = db::inbox::list_unread(chat, insider, false, 50, None)
        .await
        .unwrap();
    assert!(
        insider_rows.iter().any(|r| r.room_id == room_id),
        "the enclave member should still see their own unread message"
    );

    let outsider_rows = db::inbox::list_unread(chat, outsider, false, 50, None)
        .await
        .unwrap();
    assert!(
        !outsider_rows.iter().any(|r| r.room_id == room_id),
        "a non-member must not see the room at all"
    );
    assert!(
        !outsider_rows
            .iter()
            .any(|r| r.body.contains("the secret message body")),
        "a non-member must not receive the message body"
    );
    assert!(
        !outsider_rows
            .iter()
            .any(|r| r.room_name == "secret-general"),
        "a non-member must not receive the room name"
    );
}

#[tokio::test]
async fn dashboard_counts_hide_rooms_from_enclaves_the_viewer_is_not_in() {
    let fx = fixture().await;
    let (chat, insider, outsider, room_id) = (&fx.chat, &fx.insider, &fx.outsider, fx.room_id);

    let insider_counts = db::chat::list_room_unread_counts(chat, insider, false)
        .await
        .unwrap();
    assert!(
        insider_counts.iter().any(|(id, _)| *id == room_id),
        "the enclave member should still get a count for their room"
    );

    let outsider_counts = db::chat::list_room_unread_counts(chat, outsider, false)
        .await
        .unwrap();
    assert!(
        !outsider_counts.iter().any(|(id, _)| *id == room_id),
        "a non-member must not get a count for the room"
    );
    // This is what drives `show_dashboard` in routes/home.rs, so an outsider
    // with nothing else visible should fall through to the empty state.
    assert!(
        outsider_counts.is_empty(),
        "an outsider with no accessible room should have no counts at all"
    );
}

#[tokio::test]
async fn private_room_still_needs_room_membership_within_the_enclave() {
    // Enclave membership alone must not unlock a private channel - the fix must
    // not widen access while narrowing it elsewhere.
    let fx = fixture().await;
    let (chat, insider, outsider, enclave_id) =
        (&fx.chat, &fx.insider, &fx.outsider, fx.enclave_id);

    db::enclave::add_member(
        chat,
        enclave_id,
        outsider,
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();

    let private_id =
        db::chat::create_room(chat, "leadership", None, "private", None, Some(enclave_id))
            .await
            .unwrap();
    db::chat::insert_message(chat, private_id, insider, "private channel body")
        .await
        .unwrap();

    let rows = db::inbox::list_unread(chat, outsider, false, 50, None)
        .await
        .unwrap();
    assert!(
        !rows.iter().any(|r| r.room_id == private_id),
        "an enclave member who is not in the private room must not see it"
    );

    // ...while the public room in the same enclave is now visible to them.
    let public_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.room_name == "secret-general")
        .collect();
    assert!(
        !public_rows.is_empty(),
        "joining the enclave should reveal its public room"
    );
}

#[tokio::test]
async fn site_admin_still_sees_every_channel() {
    // The admin branch is deliberate (documented in is_room_accessible) and must
    // survive the change.
    let fx = fixture().await;
    let (chat, outsider, room_id) = (&fx.chat, &fx.outsider, fx.room_id);

    let rows = db::inbox::list_unread(chat, outsider, true, 50, None)
        .await
        .unwrap();
    assert!(
        rows.iter().any(|r| r.room_id == room_id),
        "a site admin sees channels regardless of enclave membership"
    );

    let counts = db::chat::list_room_unread_counts(chat, outsider, true)
        .await
        .unwrap();
    assert!(
        counts.iter().any(|(id, _)| *id == room_id),
        "a site admin gets counts for every channel"
    );
}
