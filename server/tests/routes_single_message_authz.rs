//! LC-636: `GET /messages/{id}` must enforce room access. A message id alone
//! must not let an authenticated non-member read a message from a room they
//! cannot see. Mirrors the `get_message_raw` gate exercised in
//! `routes_reactions_authz.rs`.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-single-msg-authz-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

async fn get_message(app: &Router, sess: &str, message_id: i64) -> StatusCode {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/messages/{message_id}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

struct Setup {
    app: Router,
    member_session: String,
    outsider_session: String,
    general_msg: i64,
    private_msg: i64,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    let outsider_id = db::auth::create_user(&auth, "outsider", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id IN (?, ?)")
        .bind(&member_id)
        .bind(&outsider_id)
        .execute(&auth)
        .await
        .unwrap();
    // Seeds the General enclave + room 1 and adds every existing user, so member
    // and outsider both become General members.
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let general_msg = db::chat::insert_message(&chat, 1, &admin_id, "hello general")
        .await
        .unwrap();

    // A standalone private room the outsider is NOT a member of.
    let priv_room = db::chat::create_room(&chat, "secret", None, "private", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, priv_room, &admin_id)
        .await
        .unwrap();
    let private_msg = db::chat::insert_message(&chat, priv_room, &admin_id, "top secret")
        .await
        .unwrap();

    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    let outsider_session = db::auth::create_session(&auth, &outsider_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
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
    Setup {
        app: routes::build_router(state),
        member_session,
        outsider_session,
        general_msg,
        private_msg,
    }
}

// A member of the room may fetch the single-message fragment.
#[tokio::test]
async fn member_can_fetch_message_in_accessible_room() {
    let s = setup().await;
    assert_eq!(
        get_message(&s.app, &s.member_session, s.general_msg).await,
        StatusCode::OK
    );
}

// The vuln: a non-member must not read a message in a room they cannot see.
#[tokio::test]
async fn non_member_is_forbidden_in_private_room() {
    let s = setup().await;
    assert_eq!(
        get_message(&s.app, &s.outsider_session, s.private_msg).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn missing_message_is_not_found() {
    let s = setup().await;
    assert_eq!(
        get_message(&s.app, &s.member_session, 999_999).await,
        StatusCode::NOT_FOUND
    );
}
