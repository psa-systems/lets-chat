//! LC-1059: Discord-style welcome message when a user joins an enclave. Covers
//! the three join triggers, the welcome-room resolution (default / configured /
//! off / stale), the manager-gated settings POST, and the wave-button render.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-welcome-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

struct TestApp {
    app: Router,
    chat: SqlitePool,
    admin_session: String,
    newbie_session: String,
    newbie_id: String,
    /// A fresh enclave owned by `admin`; `newbie` is not a member.
    enclave_id: i64,
    /// Public text rooms of `enclave_id`: "zeta" then "alpha" (alpha sorts first).
    zeta: i64,
    alpha: i64,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let newbie_id = db::auth::create_user(&auth, "newbie", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&newbie_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let newbie_session = db::auth::create_session(&auth, &newbie_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let enclave_id = db::enclave::create_enclave(&chat, "Guild", None, &admin_id)
        .await
        .unwrap();
    let zeta = db::chat::create_room(&chat, "zeta", None, "public", None, Some(enclave_id))
        .await
        .unwrap();
    let alpha = db::chat::create_room(&chat, "alpha", None, "public", None, Some(enclave_id))
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat: chat.clone(),
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
    TestApp {
        app: routes::build_router(state),
        chat,
        admin_session,
        newbie_session,
        newbie_id,
        enclave_id,
        zeta,
        alpha,
    }
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn get_html(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Every system message as (room_id, user_id, body).
async fn system_messages(chat: &SqlitePool) -> Vec<(i64, String, String)> {
    sqlx::query("SELECT room_id, user_id, body FROM messages WHERE is_system = 1 ORDER BY id")
        .fetch_all(chat)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("room_id"), r.get("user_id"), r.get("body")))
        .collect()
}

async fn join_public(t: &TestApp) -> StatusCode {
    db::enclave::set_public(&t.chat, t.enclave_id, true)
        .await
        .unwrap();
    send(
        &t.app,
        &t.newbie_session,
        Method::POST,
        &format!("/enclaves/discover/{}/join", t.enclave_id),
        "",
    )
    .await
}

#[tokio::test]
async fn discover_join_posts_one_welcome_in_default_room() {
    let t = app().await;
    assert_eq!(join_public(&t).await, StatusCode::SEE_OTHER);
    let msgs = system_messages(&t.chat).await;
    assert_eq!(msgs.len(), 1, "exactly one welcome: {msgs:?}");
    let (room_id, user_id, body) = &msgs[0];
    assert_eq!(
        *room_id, t.alpha,
        "default room is the first public room by name"
    );
    assert_eq!(user_id, &t.newbie_id);
    assert!(db::enclave_welcome::is_welcome_line(body), "body {body:?}");

    // Re-joining as an existing member posts nothing more.
    assert_eq!(join_public(&t).await, StatusCode::SEE_OTHER);
    assert_eq!(system_messages(&t.chat).await.len(), 1);
}

#[tokio::test]
async fn join_by_code_posts_welcome() {
    let t = app().await;
    sqlx::query("UPDATE enclaves SET invite_code='WELCOME1' WHERE id=?")
        .bind(t.enclave_id)
        .execute(&t.chat)
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.newbie_session,
        Method::POST,
        "/enclaves/join",
        "code=WELCOME1",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let msgs = system_messages(&t.chat).await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].1, t.newbie_id);
}

#[tokio::test]
async fn invitation_accept_posts_welcome_but_not_for_existing_member() {
    let t = app().await;
    let owner = db::enclave::get_enclave(&t.chat, t.enclave_id)
        .await
        .unwrap()
        .unwrap()
        .created_by;
    let inv = db::enclave::create_invitation(&t.chat, t.enclave_id, &t.newbie_id, &owner)
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.newbie_session,
        Method::POST,
        &format!("/invitations/{inv}/accept"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(system_messages(&t.chat).await.len(), 1);

    // A stale invite accepted by someone already in the enclave stays silent.
    let inv2 = db::enclave::create_invitation(&t.chat, t.enclave_id, &t.newbie_id, &owner)
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.newbie_session,
        Method::POST,
        &format!("/invitations/{inv2}/accept"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(system_messages(&t.chat).await.len(), 1);
}

#[tokio::test]
async fn general_backfill_posts_no_welcome() {
    let t = app().await;
    // The harness already ran backfill_general_membership for both users.
    assert!(system_messages(&t.chat).await.is_empty());
}

#[tokio::test]
async fn configured_room_and_off_are_respected() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/enclave/{}/welcome", t.enclave_id),
        &format!("room={}", t.zeta),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        db::enclave_welcome::resolve_room(&t.chat, t.enclave_id)
            .await
            .unwrap(),
        Some(t.zeta)
    );
    join_public(&t).await;
    let msgs = system_messages(&t.chat).await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].0, t.zeta, "welcome lands in the configured room");

    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/enclave/{}/welcome", t.enclave_id),
        "room=off",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        db::enclave_welcome::resolve_room(&t.chat, t.enclave_id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn off_posts_nothing_on_join() {
    let t = app().await;
    db::enclave_welcome::set(
        &t.chat,
        t.enclave_id,
        db::enclave_welcome::WelcomeConfig {
            enabled: false,
            room_id: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(join_public(&t).await, StatusCode::SEE_OTHER);
    assert!(system_messages(&t.chat).await.is_empty());
}

#[tokio::test]
async fn stale_configured_room_falls_back_and_voice_is_skipped() {
    let t = app().await;
    db::enclave_welcome::set(
        &t.chat,
        t.enclave_id,
        db::enclave_welcome::WelcomeConfig {
            enabled: true,
            room_id: Some(t.zeta),
        },
    )
    .await
    .unwrap();
    // zeta goes private: no longer eligible, fall back to the default (alpha).
    sqlx::query("UPDATE rooms SET room_type='private' WHERE id=?")
        .bind(t.zeta)
        .execute(&t.chat)
        .await
        .unwrap();
    assert_eq!(
        db::enclave_welcome::resolve_room(&t.chat, t.enclave_id)
            .await
            .unwrap(),
        Some(t.alpha)
    );
    // alpha becomes a voice channel: no eligible room left at all.
    sqlx::query("UPDATE rooms SET is_voice=1 WHERE id=?")
        .bind(t.alpha)
        .execute(&t.chat)
        .await
        .unwrap();
    assert_eq!(
        db::enclave_welcome::resolve_room(&t.chat, t.enclave_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(join_public(&t).await, StatusCode::SEE_OTHER);
    assert!(system_messages(&t.chat).await.is_empty());
}

#[tokio::test]
async fn welcome_setting_is_manager_only_and_validates_room() {
    let t = app().await;
    db::enclave::set_public(&t.chat, t.enclave_id, true)
        .await
        .unwrap();
    // A plain member (not a manager) cannot change it.
    send(
        &t.app,
        &t.newbie_session,
        Method::POST,
        &format!("/enclaves/discover/{}/join", t.enclave_id),
        "",
    )
    .await;
    let status = send(
        &t.app,
        &t.newbie_session,
        Method::POST,
        &format!("/enclave/{}/welcome", t.enclave_id),
        "room=off",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A room from another enclave (General's room 1) is rejected.
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/enclave/{}/welcome", t.enclave_id),
        "room=1",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/enclave/{}/welcome", t.enclave_id),
        "room=nope",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let cfg = db::enclave_welcome::get(&t.chat, t.enclave_id)
        .await
        .unwrap();
    assert_eq!(cfg, db::enclave_welcome::WelcomeConfig::default());
}

#[tokio::test]
async fn settings_page_renders_welcome_picker() {
    let t = app().await;
    let (status, body) = get_html(
        &t.app,
        &t.admin_session,
        &format!("/enclave/{}/settings", t.enclave_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-lc-welcome-room"));
    assert!(body.contains(r#"<option value="default" selected>"#));
    assert!(body.contains(&format!(r#"<option value="{}">#alpha</option>"#, t.alpha)));
}

#[tokio::test]
async fn wave_button_shows_for_others_not_the_newcomer() {
    let t = app().await;
    join_public(&t).await;
    let room = format!("/room/{}", t.alpha);

    let (status, body) = get_html(&t.app, &t.admin_session, &room).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("lc-sysevent--welcome"), "welcome row renders");
    assert!(body.contains("data-lc-welcome-wave"), "others see the wave");

    let (status, body) = get_html(&t.app, &t.newbie_session, &room).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("lc-sysevent--welcome"));
    assert!(!body.contains("data-lc-welcome-wave"), "no self-wave");
}

#[tokio::test]
async fn waving_adds_a_wave_reaction() {
    let t = app().await;
    join_public(&t).await;
    let msg_id: i64 = sqlx::query("SELECT id FROM messages WHERE is_system = 1")
        .fetch_one(&t.chat)
        .await
        .unwrap()
        .get("id");
    let uri = format!(
        "/messages/{msg_id}/reactions/{}",
        percent_encoding::utf8_percent_encode("👋", percent_encoding::NON_ALPHANUMERIC)
    );
    let status = send(&t.app, &t.admin_session, Method::POST, &uri, "").await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get_html(&t.app, &t.admin_session, &format!("/room/{}", t.alpha)).await;
    assert!(
        body.contains(&format!(r#"id="reactions-{msg_id}""#)) && body.contains("👋 1"),
        "wave shows in the welcome's reaction bar"
    );
}
