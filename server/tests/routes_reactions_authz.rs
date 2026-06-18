//! LC-149: reaction handlers must enforce room access. A non-member must not
//! be able to toggle a reaction on, or open the emoji picker for, a message in
//! a room they cannot see; and the `{emoji}` path segment must be bounded.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-react-authz-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

async fn react(app: &Router, sess: &str, message_id: i64, emoji: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/messages/{message_id}/reactions/{emoji}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn picker(app: &Router, sess: &str, message_id: i64) -> StatusCode {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/messages/{message_id}/reactions/picker"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

// LC-282: the raw-markdown endpoint behind the "Copy text" hover action.
async fn raw(app: &Router, sess: &str, message_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/messages/{message_id}/raw"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// LC-274: the picker returns a filterable fragment; capture its body.
async fn picker_body(app: &Router, sess: &str, message_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/messages/{message_id}/reactions/picker"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// LC-266: the reaction toggle returns the re-rendered bar; capture its body.
async fn react_body(
    app: &Router,
    sess: &str,
    message_id: i64,
    emoji: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/messages/{message_id}/reactions/{emoji}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
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

    // admin (content author + member), member (regular, in General), outsider
    // (regular, NOT in the private room).
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
    // Admin must exist before backfill, which seeds the General enclave + room 1
    // and adds all existing users (member + outsider become General members).
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // A message in the General room (id 1): every General member can react.
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
    };
    Setup {
        app: routes::build_router(state),
        member_session,
        outsider_session,
        general_msg,
        private_msg,
    }
}

#[tokio::test]
async fn non_member_cannot_toggle_reaction_in_private_room() {
    let s = setup().await;
    assert_eq!(
        react(&s.app, &s.outsider_session, s.private_msg, "%F0%9F%91%8D").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn non_member_cannot_open_picker_in_private_room() {
    let s = setup().await;
    assert_eq!(
        picker(&s.app, &s.outsider_session, s.private_msg).await,
        StatusCode::FORBIDDEN
    );
}

// LC-282: GET /messages/{id}/raw returns the stored body, access-gated.
#[tokio::test]
async fn raw_returns_body_for_accessible_message() {
    let s = setup().await;
    let (status, body) = raw(&s.app, &s.member_session, s.general_msg).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello general");
}

#[tokio::test]
async fn raw_forbidden_for_inaccessible_room() {
    let s = setup().await;
    let (status, _) = raw(&s.app, &s.outsider_session, s.private_msg).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn raw_not_found_for_missing_message() {
    let s = setup().await;
    let (status, _) = raw(&s.app, &s.member_session, 999_999).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// LC-274: the picker ships a filter input and per-button searchable name tokens.
#[tokio::test]
async fn picker_ships_filter_and_name_tokens() {
    let s = setup().await;
    let (status, body) = picker_body(&s.app, &s.member_session, s.general_msg).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("data-lc-emoji-filter"),
        "picker must ship the filter input: {body}"
    );
    assert!(
        body.contains("data-lc-emoji-name="),
        "picker buttons must carry searchable name tokens: {body}"
    );
    // LC-288: the recent-emoji row placeholder (filled client-side from
    // localStorage) ships in the picker.
    assert!(
        body.contains("data-lc-emoji-recent"),
        "picker must ship the recent-emoji row placeholder: {body}"
    );
}

// LC-266: the reaction pill carries a `title` listing who reacted.
#[tokio::test]
async fn reaction_pill_titles_who_reacted() {
    let s = setup().await;
    let (status, body) = react_body(&s.app, &s.member_session, s.general_msg, "%F0%9F%91%8D").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"title="member""#),
        "reaction pill must title the reactor's name: {body}"
    );
}

#[tokio::test]
async fn member_can_toggle_reaction_in_accessible_room() {
    let s = setup().await;
    assert_eq!(
        react(&s.app, &s.member_session, s.general_msg, "%F0%9F%91%8D").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn member_can_open_picker_in_accessible_room() {
    let s = setup().await;
    assert_eq!(
        picker(&s.app, &s.member_session, s.general_msg).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn oversized_emoji_token_is_rejected() {
    let s = setup().await;
    let huge = "x".repeat(100);
    assert_eq!(
        react(&s.app, &s.member_session, s.general_msg, &huge).await,
        StatusCode::BAD_REQUEST
    );
}
