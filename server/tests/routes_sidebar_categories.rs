use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p =
                std::env::temp_dir().join(format!("lc-sidebar-cats-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    member_id: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

/// Seed two users: `admin` (enclave owner via being the first registered
/// user + General enclave bootstrap) and `member` (joined the General
/// enclave as a regular member). Returns both sessions so tests can
/// exercise RBAC on the new shared category endpoints.
async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&member_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let chat_for_test = chat.clone();
    let auth_for_test = auth.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        member_id,
        auth: auth_for_test,
        chat: chat_for_test,
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

async fn category_count(chat: &SqlitePool, enclave_id: i64) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM room_categories WHERE enclave_id = ?")
        .bind(enclave_id)
        .fetch_one(chat)
        .await
        .unwrap();
    row.get::<i64, _>("n")
}

async fn assignment_count(chat: &SqlitePool) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM room_category_assignments")
        .fetch_one(chat)
        .await
        .unwrap();
    row.get::<i64, _>("n")
}

#[tokio::test]
async fn admin_creates_category() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/enclave/1/sidebar/categories",
        "name=Work",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(category_count(&t.chat, 1).await, 1);
}

#[tokio::test]
async fn member_cannot_create_category() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/enclave/1/sidebar/categories",
        "name=Work",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(category_count(&t.chat, 1).await, 0);
}

#[tokio::test]
async fn admin_assigns_room_then_member_sees_it_in_category() {
    let t = app().await;
    let cat = db::sidebar_categories::create_category(&t.chat, 1, "Work")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        &format!("/enclave/1/sidebar/categories/{cat}/rooms/1"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assignment_count(&t.chat).await, 1);

    // Member opening the enclave sees the General room inside the category
    // section, not in "All rooms". LC-143: /enclave/1 redirects to a room;
    // the room page's sidebar carries the same category markup, so follow it.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/enclave/1")
        .header(header::COOKIE, format!("session={}", t.member_session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let target = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("redirect to a room")
        .to_string();
    let req = Request::builder()
        .method(Method::GET)
        .uri(target)
        .header(header::COOKIE, format!("session={}", t.member_session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), 10 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let cat_marker = body
        .find(&format!("data-category-id=\"{cat}\""))
        .expect("category section in HTML");
    let room_link = body.find("/room/1\"").expect("room link in HTML");
    // Room link must appear after the category marker (inside it),
    // before any "All rooms" header.
    let all_rooms = body.find(">All rooms<");
    assert!(room_link > cat_marker, "room link before category section");
    if let Some(ar) = all_rooms {
        assert!(room_link < ar, "room ended up in All rooms");
    }
}

#[tokio::test]
async fn member_collapse_only_affects_self() {
    let t = app().await;
    let cat = db::sidebar_categories::create_category(&t.chat, 1, "Work")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.member_session,
        Method::PATCH,
        &format!("/sidebar/categories/{cat}/collapse"),
        "collapsed=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let collapsed = db::sidebar_categories::list_collapsed_for_user(&t.auth, &t.member_id)
        .await
        .unwrap();
    assert!(collapsed.contains(&cat));
}
