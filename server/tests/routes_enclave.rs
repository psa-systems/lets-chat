use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
            include_str!("../migrations/auth/0001_create_tables.sql"),
            include_str!("../migrations/auth/0002_read_receipts.sql"),
        ],
        "chat" => vec![
            include_str!("../migrations/chat/0001_create_tables.sql"),
            include_str!("../migrations/chat/0002_moderation.sql"),
            include_str!("../migrations/chat/0003_dms.sql"),
            include_str!("../migrations/chat/0004_message_editing.sql"),
            include_str!("../migrations/chat/0005_private_rooms.sql"),
            include_str!("../migrations/chat/0006_read_receipts.sql"),
            include_str!("../migrations/chat/0007_reactions.sql"),
            include_str!("../migrations/chat/0008_search.sql"),
            include_str!("../migrations/chat/0009_enclaves.sql"),
        ],
        "settings" => vec![include_str!(
            "../migrations/settings/0001_create_tables.sql"
        )],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

pub async fn app_with_user(role: &str) -> (Router, String) {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, "tester", "hash").await.unwrap();
    sqlx::query("UPDATE users SET role=? WHERE id=?")
        .bind(role)
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session_token = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
    };
    let app = routes::build_router(state);
    (app, session_token)
}

pub fn cookie(token: &str) -> String {
    format!("session={token}")
}

#[tokio::test]
async fn post_enclaves_creates_and_redirects() {
    let (app, sess) = app_with_user("user").await;
    let body = "name=rust&description=rustaceans";
    let req = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.starts_with("/enclave/"));
}

#[tokio::test]
async fn get_enclave_landing_renders_for_member() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=rust"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let loc = res.headers().get("location").unwrap().to_str().unwrap().to_string();

    let req = Request::builder()
        .method(Method::GET)
        .uri(&loc)
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(s.contains("rust"));
}

#[tokio::test]
async fn get_enclave_landing_404_for_unknown() {
    let (app, sess) = app_with_user("user").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/enclave/999999")
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_enclaves_requires_auth() {
    let (app, _sess) = app_with_user("user").await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=x"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(res.status().is_redirection() || res.status() == StatusCode::SEE_OTHER);
}
