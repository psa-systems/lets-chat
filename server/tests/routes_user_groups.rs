use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-groups-tests-{}", std::process::id()));
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
    chat: SqlitePool,
    auth: SqlitePool,
}

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
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        member_id,
        chat: chat_for_test,
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

#[tokio::test]
async fn admin_creates_group_and_adds_member() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/enclave/1/groups",
        "name=designers",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let group_id: i64 = sqlx::query_scalar("SELECT id FROM user_groups WHERE name='designers'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let body = format!("user_id={}", t.member_id);
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/enclave/1/groups/{group_id}/members"),
        &body,
    )
    .await;
    // Typeahead row swap: handler returns 200 + an HTML fragment, not a redirect.
    assert_eq!(status, StatusCode::OK);
    let members = db::user_groups::list_member_ids(&t.chat, group_id)
        .await
        .unwrap();
    assert_eq!(members, vec![t.member_id.clone()]);
}

#[tokio::test]
async fn member_cannot_create_group() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/enclave/1/groups",
        "name=sre",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_groups WHERE enclave_id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(cnt, 0);
}

#[tokio::test]
async fn group_mention_expands_to_member_mention_rows() {
    let t = app().await;
    // Create group, add member.
    let group_id = db::user_groups::create(&t.chat, 1, "designers", None, "admin_id")
        .await
        .unwrap();
    db::user_groups::add_member(&t.chat, group_id, &t.member_id)
        .await
        .unwrap();

    // Admin sends a message with @designers. Mention parser should
    // expand to a `mentions` row for the member.
    let body = "body=hey+@designers&file_id=";
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/messages")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(body))
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let row: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions WHERE mentioned_user_id = ?")
        .bind(&t.member_id)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(row, 1);
    let _ = t.auth;
}
