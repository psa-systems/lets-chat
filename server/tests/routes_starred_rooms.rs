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
            let p = std::env::temp_dir().join(format!("lc-stars-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    session: String,
    user_id: String,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user_id = db::auth::create_user(&auth, "viewer", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
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
        session,
        user_id,
        auth: auth_for_test,
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

async fn star_count(auth: &SqlitePool, user_id: &str) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM starred_rooms WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(auth)
        .await
        .unwrap();
    row.get::<i64, _>("n")
}

#[tokio::test]
async fn toggle_star_inserts_then_removes_row() {
    let t = app().await;
    assert_eq!(star_count(&t.auth, &t.user_id).await, 0);

    let status = send(&t.app, &t.session, Method::POST, "/rooms/1/star", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(star_count(&t.auth, &t.user_id).await, 1);

    let status = send(&t.app, &t.session, Method::POST, "/rooms/1/star", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(star_count(&t.auth, &t.user_id).await, 0);
}

#[tokio::test]
async fn toggle_star_refuses_inaccessible_room() {
    let t = app().await;
    let status = send(&t.app, &t.session, Method::POST, "/rooms/999/star", "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(star_count(&t.auth, &t.user_id).await, 0);
}

#[tokio::test]
async fn positions_endpoint_reorders() {
    let t = app().await;
    // Seed multiple star rows directly via the DB layer (room ids
    // 1..=3; only room 1 is real, but the stars table doesn't FK-check
    // chat.db).
    for room_id in [1_i64, 2, 3] {
        db::starred_rooms::star(&t.auth, &t.user_id, room_id)
            .await
            .unwrap();
    }
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        "/sidebar/stars/positions",
        "ids=3,1,2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let positions = db::starred_rooms::star_positions(&t.auth, &t.user_id)
        .await
        .unwrap();
    assert_eq!(positions.get(&3), Some(&0));
    assert_eq!(positions.get(&1), Some(&1));
    assert_eq!(positions.get(&2), Some(&2));
}
