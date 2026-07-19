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
    admin_id: String,
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
        geoip: None,
        login_approval_enabled: false,
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
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
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

/// Like `send`, but returns the rendered response body too. Deliberately sends
/// NO `HX-Current-URL` header so the re-render cannot lean on it.
async fn send_body(
    app: &Router,
    sess: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 2_000_000)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
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

/// LC-415 regression: the re-rendered sidebar fragment must contain the new
/// category (and the add-category form) even when the request carries NO
/// `HX-Current-URL` header. The handler derives the enclave from its path
/// `enclave_id`, not the header; before the fix it fell back to the header,
/// got `None`, and returned the DM-only sidebar - so the category and the
/// add-form vanished and the user saw "nothing happened".
#[tokio::test]
async fn create_response_includes_category_without_hx_header() {
    let t = app().await;
    let (status, body) = send_body(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/enclave/1/sidebar/categories",
        "name=WorkZebra",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(category_count(&t.chat, 1).await, 1);
    assert!(
        body.contains("WorkZebra"),
        "re-rendered sidebar must show the just-created category"
    );
    assert!(
        body.contains("/enclave/1/sidebar/categories"),
        "re-rendered sidebar must still carry the enclave-scoped add-category form"
    );
}

/// LC-415 hardening: the whole-sidebar re-render routes with NO authoritative
/// enclave in their path (read-all, star toggle/reorder, mark-unread) keep the
/// viewer's current-enclave context via the explicit `X-LC-Current-Enclave`
/// header that live.js sends from the rendered `#sidebar-nav-{id}`. Exercise it
/// through `/read-all`: with the header the enclave sidebar (category +
/// add-form) survives the re-render; with no context header at all it falls
/// back to the DM-only shape (the control that proves the header is what
/// preserved it).
#[tokio::test]
async fn read_all_preserves_enclave_via_explicit_header() {
    let t = app().await;
    db::sidebar_categories::create_category(&t.chat, 1, "WorkZebra")
        .await
        .unwrap();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/read-all")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .header("X-LC-Current-Enclave", "1")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), 2_000_000)
            .await
            .unwrap(),
    )
    .to_string();
    assert!(
        body.contains("WorkZebra"),
        "explicit X-LC-Current-Enclave header must preserve enclave context"
    );
    assert!(
        body.contains("/enclave/1/sidebar/categories"),
        "enclave add-category form present after read-all"
    );

    // Control: no context header of any kind -> DM-only sidebar.
    let (status, body2) = send_body(&t.app, &t.admin_session, Method::POST, "/read-all", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body2.contains("WorkZebra"),
        "with no current-enclave signal the re-render is the DM-only sidebar"
    );
}

/// LC-415 hardening: a crafted `X-LC-Current-Enclave` for an enclave the viewer
/// is not a member of must NOT leak that enclave's category names. `member` is
/// only in the General enclave (id 1); pointing the header at a foreign enclave
/// the admin created falls back to the DM-only sidebar.
#[tokio::test]
async fn spoofed_current_enclave_header_does_not_leak_categories() {
    let t = app().await;
    // A second enclave the `member` user is NOT in, owned by `admin`.
    let other = db::enclave::create_enclave(&t.chat, "Secret", None, &t.admin_id)
        .await
        .unwrap();
    db::sidebar_categories::create_category(&t.chat, other, "TopSecretCat")
        .await
        .unwrap();

    let req = Request::builder()
        .method(Method::POST)
        .uri("/read-all")
        .header(header::COOKIE, format!("session={}", t.member_session))
        .header("X-LC-Current-Enclave", other.to_string())
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), 2_000_000)
            .await
            .unwrap(),
    )
    .to_string();
    assert!(
        !body.contains("TopSecretCat"),
        "non-member must not see a spoofed enclave's category names"
    );
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
