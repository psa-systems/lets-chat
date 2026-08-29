//! LC-838: a live WebSocket authorizes against the user's CURRENT record, not
//! the snapshot taken when the socket opened.
//!
//! Until LC-837 every navigation opened a fresh socket, so a role or preference
//! change reached the socket on the user's next page move. The socket now
//! outlives navigation, and `handle_socket`'s `User` would have been frozen
//! for the session: a demoted admin keeping the `admin` topic and non-member
//! enclave reads, a user who turned read receipts off still emitting "Seen"
//! captions, a rename never reaching a relayed call invite.
//!
//! The fix re-reads the record on the `page_context` frame (`refresh_account`),
//! which is exactly where a fresh socket used to take it, and both loops read
//! the refreshed snapshot. These tests drive `refresh_account` directly and
//! assert the gates the ticket names follow it; the first one also pins the
//! frozen behaviour it replaces, so the refresh is demonstrably what changes
//! the answer.
use lets_chat::models::User;
use lets_chat::push::{MockPushClient, PushClient};
use lets_chat::routes::test_support::{
    refresh_account, relay_call_signal, render_dm_read, topic_subscribe_allowed, Account,
};
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, state::AppState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod common;

struct Fx {
    state: AppState,
    hub: Arc<Hub>,
    user_id: String,
    peer_id: String,
    /// An enclave the user is NOT a member of: only an admin may read it.
    foreign_enclave: i64,
    dm_room: i64,
}

async fn fixture() -> Fx {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let user_id = db::auth::create_user(&auth, "navigator", "hash")
        .await
        .unwrap();
    let peer_id = db::auth::create_user(&auth, "peer", "hash").await.unwrap();
    let foreign_enclave = db::enclave::create_enclave(&chat, "Not Yours", None, &peer_id)
        .await
        .unwrap();
    let dm_room = db::chat::create_dm_room(&chat, "dm", &user_id, &peer_id)
        .await
        .unwrap()
        .id;

    let hub = Arc::new(Hub::new());
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: hub.clone(),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(MockPushClient::default()) as Arc<dyn PushClient>,
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    Fx {
        state,
        hub,
        user_id,
        peer_id,
        foreign_enclave,
        dm_room,
    }
}

/// The record as the HTTP upgrade would hand it to `handle_socket`, right now.
async fn load(fx: &Fx) -> User {
    db::auth::find_user_by_id(&fx.state.auth, &fx.user_id)
        .await
        .unwrap()
        .unwrap()
        .into()
}

/// What `handle_socket` holds: the connect-time snapshot behind the shared
/// `Account`, plus the hub registration under its display label.
async fn connect(fx: &Fx) -> (u64, tokio::sync::broadcast::Receiver<ChatEvent>, Account) {
    let user = load(fx).await;
    let (conn_id, rx, _) = fx.hub.connect(&user.id, user.display_label());
    (conn_id, rx, Arc::new(Mutex::new(Arc::new(user))))
}

fn snapshot(account: &Account) -> Arc<User> {
    account.lock().unwrap().clone()
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_demoted_admin_is_refused_on_the_next_refresh_without_closing_the_socket() {
    let fx = fixture().await;
    db::auth::set_user_role(&fx.state.auth, &fx.user_id, "admin")
        .await
        .unwrap();
    let (conn_id, _rx, account) = connect(&fx).await;
    let foreign = format!("enclave:{}", fx.foreign_enclave);

    // Admin at connect: the `admin` topic and the non-member enclave read.
    let user = snapshot(&account);
    assert!(topic_subscribe_allowed(&fx.state, &user, "admin").await);
    assert!(topic_subscribe_allowed(&fx.state, &user, &foreign).await);

    // Demoted mid-connection. The snapshot in hand is the frozen one LC-838
    // replaces: it still says admin. Pinned so the refresh below is what
    // changes the answer, not the demotion alone.
    db::auth::set_user_role(&fx.state.auth, &fx.user_id, "user")
        .await
        .unwrap();
    assert!(
        topic_subscribe_allowed(&fx.state, &user, "admin").await,
        "a snapshot taken at connect cannot see the demotion; that is the defect"
    );

    // The next page move re-reads the record. Socket stays open.
    let fresh = refresh_account(&fx.state, conn_id, &account)
        .await
        .expect("the account still exists, so the socket stays open");
    assert_eq!(fresh.role, "user");
    assert!(
        !topic_subscribe_allowed(&fx.state, &fresh, "admin").await,
        "the admin topic is refused after the refresh"
    );
    assert!(
        !topic_subscribe_allowed(&fx.state, &fresh, &foreign).await,
        "the non-member enclave read is refused after the refresh"
    );
    // And the shared snapshot both loops read is the refreshed one.
    assert_eq!(snapshot(&account).role, "user");
}

#[tokio::test]
async fn read_receipts_turned_off_mid_connection_stop_the_seen_caption() {
    let fx = fixture().await;
    let (conn_id, _rx, account) = connect(&fx).await;
    let dm_seen: Arc<Mutex<HashMap<i64, i64>>> = Arc::default();

    // The user wrote a message; the peer (receipts on, the default) read it.
    let msg = db::chat::insert_message(&fx.state.chat, fx.dm_room, &fx.user_id, "hello")
        .await
        .unwrap();
    // The peer's read watermark, which the DM page GET writes on their side and
    // `find_dm_seen_state` joins against.
    db::chat::upsert_dm_read(&fx.state.chat, &fx.peer_id, fx.dm_room, msg)
        .await
        .unwrap();
    let read_at = "2026-08-28T10:00:00Z";

    let user = snapshot(&account);
    let caption = render_dm_read(
        &fx.state,
        &user,
        fx.dm_room,
        &fx.peer_id,
        msg,
        read_at,
        &dm_seen,
    )
    .await
    .expect("with receipts on, the peer's read renders a Seen caption");
    assert!(caption.contains("Seen "), "got:\n{caption}");

    // The user turns receipts off. Their next page move refreshes the record,
    // and the same read on the same connection renders nothing.
    db::auth::set_read_receipts_enabled(&fx.state.auth, &fx.user_id, false)
        .await
        .unwrap();
    let fresh = refresh_account(&fx.state, conn_id, &account).await.unwrap();
    assert!(!fresh.read_receipts_enabled);
    dm_seen.lock().unwrap().clear();
    assert!(
        render_dm_read(
            &fx.state,
            &fresh,
            fx.dm_room,
            &fx.peer_id,
            msg,
            read_at,
            &dm_seen
        )
        .await
        .is_none(),
        "with receipts off, the same connection emits no caption"
    );
}

#[tokio::test]
async fn a_rename_reaches_the_hub_and_the_next_relayed_invite() {
    let fx = fixture().await;
    let (conn_id, _rx, account) = connect(&fx).await;
    let (_peer_conn, mut peer_rx, _) = fx.hub.connect(&fx.peer_id, "peer");
    assert_eq!(fx.hub.username_of(conn_id).as_deref(), Some("navigator"));

    // An invite before the rename carries the connect-time label, as the
    // receive loop passes `user.display_label()` along.
    let user = snapshot(&account);
    relay_call_signal(
        &fx.state,
        &user,
        user.display_label(),
        fx.dm_room,
        "invite",
        None,
    )
    .await;
    let from = |ev: &ChatEvent| match ev {
        ChatEvent::CallSignal { from_name, .. } => Some(from_name.clone()),
        _ => None,
    };
    let mut names = Vec::new();
    while let Ok(ev) = peer_rx.try_recv() {
        if let Some(n) = from(&ev) {
            names.push(n);
        }
    }
    assert_eq!(names, vec!["navigator".to_string()]);
    fx.hub.clear_ringing(fx.dm_room);

    // Renamed mid-connection, then the next page move.
    db::auth::update_user_profile(
        &fx.state.auth,
        &fx.user_id,
        Some("Nav Igator"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let fresh = refresh_account(&fx.state, conn_id, &account).await.unwrap();
    assert_eq!(fresh.display_label(), "Nav Igator");
    assert_eq!(
        fx.hub.username_of(conn_id).as_deref(),
        Some("Nav Igator"),
        "Hub::connections carries its own copy of the label and must be renamed too"
    );

    relay_call_signal(
        &fx.state,
        &fresh,
        fresh.display_label(),
        fx.dm_room,
        "invite",
        None,
    )
    .await;
    let mut names = Vec::new();
    while let Ok(ev) = peer_rx.try_recv() {
        if let Some(n) = from(&ev) {
            names.push(n);
        }
    }
    assert_eq!(
        names,
        vec!["Nav Igator".to_string()],
        "the invite after the refresh carries the new name"
    );
}

#[tokio::test]
async fn a_deleted_account_ends_the_refresh_so_the_socket_closes() {
    let fx = fixture().await;
    let (conn_id, _rx, account) = connect(&fx).await;
    db::auth::delete_user(&fx.state.auth, &fx.user_id)
        .await
        .unwrap();
    assert!(
        refresh_account(&fx.state, conn_id, &account)
            .await
            .is_none(),
        "no record, no user to authorize: the receive loop breaks and the socket closes"
    );
}
