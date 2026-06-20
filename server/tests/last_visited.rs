use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

async fn app_with_user_in_general() -> (Router, String, String) {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, "tester", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

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
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
    };
    (routes::build_router(state), session, user_id)
}

#[tokio::test]
async fn home_renders_welcome_when_no_cookie() {
    let (app, sess, _id) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("cookie", format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(s.contains("Welcome"), "should render welcome page");
}

// LC-372: the welcome empty state renders the quick-action cards (Start a DM,
// Discover/create an enclave, View invitations) and the supporting subtitle.
#[tokio::test]
async fn home_welcome_renders_quick_actions() {
    let (app, sess, _id) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("cookie", format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        s.contains("start something new"),
        "renders the empty-state subtitle"
    );
    assert!(
        s.contains("Start a direct message"),
        "renders the Start-a-DM quick action"
    );
    assert!(
        s.contains("href=\"/enclaves/discover\""),
        "renders the discover/create enclave action with its link"
    );
    assert!(
        s.contains("href=\"/invitations\""),
        "renders the view-invitations action with its link"
    );
}

#[tokio::test]
async fn home_redirects_to_last_visited_room_when_safe() {
    let (app, sess, _id) = app_with_user_in_general().await;
    // The migration seeded 'general' (id=1) and 'random' (id=2) into the General enclave.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(
            "cookie",
            format!("session={sess}; lets_chat_last_visited=/room/1"),
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap().to_str().unwrap(),
        "/room/1"
    );
}

#[tokio::test]
async fn home_ignores_malformed_cookie() {
    let (app, sess, _id) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(
            "cookie",
            format!("session={sess}; lets_chat_last_visited=/admin/users"),
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn home_ignores_inaccessible_room_cookie() {
    let (app, sess, _id) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(
            "cookie",
            format!("session={sess}; lets_chat_last_visited=/room/999999"),
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_room_sets_last_visited_cookie() {
    let (app, sess, _id) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/room/1")
        .header("cookie", format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie_header = res.headers().get_all("set-cookie");
    let mut found = false;
    for v in cookie_header {
        if v.to_str()
            .unwrap()
            .contains("lets_chat_last_visited=/room/1")
        {
            found = true;
        }
    }
    assert!(
        found,
        "Set-Cookie header must include the last_visited entry"
    );
}
