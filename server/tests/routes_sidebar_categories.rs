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
    session: String,
    user_id: String,
    auth: SqlitePool,
}

/// Seed an admin user with the General enclave bootstrapped (so room id 1
/// exists and is accessible). totp_enabled = 1 to bypass the 2FA-enrollment
/// middleware that the test harness pattern (see CLAUDE.md test maintenance
/// section 5) otherwise redirects every authed request through.
async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user_id = db::auth::create_user(&auth, "viewer", "hash")
        .await
        .unwrap();
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
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
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

async fn category_count(auth: &SqlitePool, user_id: &str) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM sidebar_categories WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(auth)
        .await
        .unwrap();
    row.get::<i64, _>("n")
}

async fn assignment_count(auth: &SqlitePool, user_id: &str) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM sidebar_category_rooms WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(auth)
        .await
        .unwrap();
    row.get::<i64, _>("n")
}

#[tokio::test]
async fn create_category_persists_row() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.session,
        Method::POST,
        "/sidebar/categories",
        "name=Work",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(category_count(&t.auth, &t.user_id).await, 1);
}

#[tokio::test]
async fn create_category_rejects_empty_name() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.session,
        Method::POST,
        "/sidebar/categories",
        "name=",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(category_count(&t.auth, &t.user_id).await, 0);
}

#[tokio::test]
async fn rename_then_delete_category() {
    let t = app().await;
    let id = db::sidebar_categories::create_category(&t.auth, &t.user_id, "Work")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        &format!("/sidebar/categories/{id}"),
        "name=Projects",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = sqlx::query("SELECT name FROM sidebar_categories WHERE id = ?")
        .bind(id)
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("name"), "Projects");

    let status = send(
        &t.app,
        &t.session,
        Method::DELETE,
        &format!("/sidebar/categories/{id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(category_count(&t.auth, &t.user_id).await, 0);
}

#[tokio::test]
async fn assign_room_then_delete_cascade_clears_assignment() {
    let t = app().await;
    let id = db::sidebar_categories::create_category(&t.auth, &t.user_id, "Work")
        .await
        .unwrap();
    // Room id 1 = the seeded General enclave room. The viewer is an admin
    // so is_room_accessible passes.
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        &format!("/sidebar/categories/{id}/rooms/1"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assignment_count(&t.auth, &t.user_id).await, 1);

    // Deleting the category cascades the assignment row.
    let status = send(
        &t.app,
        &t.session,
        Method::DELETE,
        &format!("/sidebar/categories/{id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assignment_count(&t.auth, &t.user_id).await, 0);
}

#[tokio::test]
async fn assign_room_requires_room_access() {
    let t = app().await;
    let id = db::sidebar_categories::create_category(&t.auth, &t.user_id, "Work")
        .await
        .unwrap();
    // Room id 999 does not exist; is_room_accessible returns false, so
    // the assignment endpoint must refuse with 403 (and persist nothing).
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        &format!("/sidebar/categories/{id}/rooms/999"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(assignment_count(&t.auth, &t.user_id).await, 0);
}

#[tokio::test]
async fn delete_room_assignment_unassigns_without_unjoining() {
    let t = app().await;
    let id = db::sidebar_categories::create_category(&t.auth, &t.user_id, "Work")
        .await
        .unwrap();
    db::sidebar_categories::assign_room(&t.auth, &t.user_id, 1, id)
        .await
        .unwrap();
    assert_eq!(assignment_count(&t.auth, &t.user_id).await, 1);

    let status = send(
        &t.app,
        &t.session,
        Method::DELETE,
        "/sidebar/categories/rooms/1",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assignment_count(&t.auth, &t.user_id).await, 0);
    // The category itself is untouched.
    assert_eq!(category_count(&t.auth, &t.user_id).await, 1);
}

#[tokio::test]
async fn category_positions_endpoint_reorders_in_place() {
    let t = app().await;
    let a = db::sidebar_categories::create_category(&t.auth, &t.user_id, "A")
        .await
        .unwrap();
    let b = db::sidebar_categories::create_category(&t.auth, &t.user_id, "B")
        .await
        .unwrap();
    let c = db::sidebar_categories::create_category(&t.auth, &t.user_id, "C")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        "/sidebar/categories/positions",
        &format!("ids={c},{a},{b}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cats = db::sidebar_categories::list_categories(&t.auth, &t.user_id)
        .await
        .unwrap();
    let order: Vec<i64> = cats.iter().map(|c| c.id).collect();
    assert_eq!(order, vec![c, a, b]);
}

#[tokio::test]
async fn room_positions_endpoint_handles_cross_category_move() {
    let t = app().await;
    let a = db::sidebar_categories::create_category(&t.auth, &t.user_id, "A")
        .await
        .unwrap();
    let b = db::sidebar_categories::create_category(&t.auth, &t.user_id, "B")
        .await
        .unwrap();
    db::sidebar_categories::assign_room(&t.auth, &t.user_id, 1, a)
        .await
        .unwrap();
    // Cross-category drag: room 1 currently in A; positions endpoint
    // for B includes it, so the assignment row gets upserted into B.
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        &format!("/sidebar/categories/{b}/positions"),
        "ids=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let assignments = db::sidebar_categories::room_assignments(&t.auth, &t.user_id)
        .await
        .unwrap();
    assert_eq!(assignments.get(&1).map(|(c, _)| *c), Some(b));
}

#[tokio::test]
async fn room_positions_endpoint_refuses_inaccessible_room() {
    let t = app().await;
    let a = db::sidebar_categories::create_category(&t.auth, &t.user_id, "A")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.session,
        Method::PATCH,
        &format!("/sidebar/categories/{a}/positions"),
        "ids=1,999",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(assignment_count(&t.auth, &t.user_id).await, 0);
}
