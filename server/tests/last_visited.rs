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
            include_str!("../migrations/auth/0003_profile_fields.sql"),
            include_str!("../migrations/auth/0004_user_status.sql"),
            include_str!("../migrations/auth/0005_profile_visibility.sql"),
            include_str!("../migrations/auth/0006_user_blocks.sql"),
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
            include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
            include_str!("../migrations/chat/0011_threads.sql"),
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

async fn app_with_user_in_general() -> (Router, String, String) {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, "tester", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
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
