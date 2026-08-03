//! LC-637: the WS `subscribe` frame must require enclave membership, the same
//! predicate the HTTP room handler uses. Before the fix, `ClientFrame::Subscribe`
//! special-cased only `dm`/`private` and treated every other `room_type` as
//! globally open, so a non-member of an enclave could subscribe to one of its
//! public rooms and receive its live message bodies. Paired with
//! `broadcast_room_message` fanning public-room events to `list_connected_users`,
//! an interloper got message fragments pushed into their socket.
//!
//! Same defect class as resolved LC-604 (the read paths), which these tests
//! mirror: they pin both the subscribe gate and the public fan-out recipient
//! set to `is_room_accessible`.
use axum::body::Body;
use axum::http::{header, Method, Request};
use axum::Router;
use lets_chat::push::{MockPushClient, PushClient};
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes, state::AppState};
use std::sync::Arc;
use tokio::sync::broadcast::error::TryRecvError;
use tower::ServiceExt;

mod common;

// ---------------------------------------------------------------------------
// Gate + recipient-set: pure DB predicates the fix routes through.
// ---------------------------------------------------------------------------

struct Fx {
    chat: sqlx::SqlitePool,
    /// Member of the enclave.
    insider: String,
    /// In no enclave at all.
    outsider: String,
    room_id: i64,
}

/// A public room inside a private enclave, seeded exactly like the LC-604
/// sibling. The freshly created enclave is not id 1 (migration 0009 seeds a
/// "General"), so ids are read back rather than assumed.
async fn fixture() -> Fx {
    let chat = common::chat_pool().await;
    let insider = "insider-user".to_string();
    let outsider = "outsider-user".to_string();

    let enclave_id = db::enclave::create_enclave(&chat, "Private Team", None, &insider)
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

    Fx {
        chat,
        insider,
        outsider,
        room_id,
    }
}

#[tokio::test]
async fn subscribe_gate_refuses_the_outsider() {
    // This is the predicate `ClientFrame::Subscribe` now calls. Before the fix
    // the frame ignored it and returned true for every non-dm/private room.
    let fx = fixture().await;

    assert!(
        db::chat::is_room_accessible(&fx.chat, fx.room_id, &fx.insider, false)
            .await
            .unwrap(),
        "an enclave member may subscribe to a public room in their enclave"
    );
    assert!(
        !db::chat::is_room_accessible(&fx.chat, fx.room_id, &fx.outsider, false)
            .await
            .unwrap(),
        "a non-member must be refused the subscribe frame for that room"
    );
    assert!(
        db::chat::is_room_accessible(&fx.chat, fx.room_id, &fx.outsider, true)
            .await
            .unwrap(),
        "a site admin keeps god-mode, matching topic_subscribe_allowed"
    );
}

#[tokio::test]
async fn public_fan_out_recipients_exclude_non_members() {
    // The recipient set `broadcast_room_message`'s public arm now uses. The
    // old arm was `list_connected_users()`, which included the outsider.
    let fx = fixture().await;

    let recipients = db::chat::list_enclave_member_ids_for_room(&fx.chat, fx.room_id)
        .await
        .unwrap();
    assert!(
        recipients.contains(&fx.insider),
        "the enclave member is a recipient of the public room's events"
    );
    assert!(
        !recipients.contains(&fx.outsider),
        "a non-member is never a recipient of the public room's events"
    );
}

// ---------------------------------------------------------------------------
// End-to-end: a real member post over the HTTP handler must not reach a
// connected non-member's socket, but must reach a connected member's.
// ---------------------------------------------------------------------------

struct App {
    app: Router,
    hub: Arc<Hub>,
    member_id: String,
    member_session: String,
    room_id: i64,
}

async fn setup_app() -> App {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    // The posting member. A plain (non-admin) account, so the fix - not the
    // admin god-mode branch - is what gates delivery.
    let member_id = db::auth::create_user(&auth, "member", "hash")
        .await
        .unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();

    // Their enclave and its public room. create_enclave adds the creator as a
    // member, so `member_id` can post; the outsider is added to no enclave.
    let enclave_id = db::enclave::create_enclave(&chat, "Team", None, &member_id)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None, Some(enclave_id))
        .await
        .unwrap();

    let hub = Arc::new(Hub::new());
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat: chat.clone(),
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
    let app = routes::build_router(state);
    App {
        app,
        hub,
        member_id,
        member_session,
        room_id,
    }
}

#[tokio::test]
async fn connected_non_member_receives_no_message_when_a_member_posts() {
    let a = setup_app().await;

    // Both hold a live socket. The outsider is in no enclave; the member is.
    let (_m_conn, mut member_rx, _) = a.hub.connect(&a.member_id, "member");
    let (_o_conn, mut outsider_rx, _) = a.hub.connect("outsider-user", "outsider");

    // The member posts to their public room through the real handler, which
    // gates on is_room_accessible and fans out via broadcast_room_message.
    let form = "body=the+secret+message+body&file_id=";
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{}/messages", a.room_id))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", a.member_session))
        .body(Body::from(form))
        .unwrap();
    let status = a.app.clone().oneshot(req).await.unwrap().status();
    assert!(
        status.is_success(),
        "the member's post should succeed, got {status}"
    );

    // The member (a recipient) receives their NewMessage render.
    let mut member_got_new_message = false;
    loop {
        match member_rx.try_recv() {
            Ok(ChatEvent::NewMessage { .. }) => {
                member_got_new_message = true;
                break;
            }
            Ok(_) => continue,
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(_)) => continue,
        }
    }
    assert!(
        member_got_new_message,
        "the enclave member must receive the NewMessage fragment"
    );

    // The non-member's socket must have received nothing at all.
    assert!(
        matches!(outsider_rx.try_recv(), Err(TryRecvError::Empty)),
        "a connected non-member must receive no event for a room they cannot access"
    );
}
