//! LC-86 integration: room info page + wiki + description endpoints.
use axum::body::{to_bytes, Body};
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
            let p = std::env::temp_dir().join(format!("lc-info-tests-{}", std::process::id()));
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
    chat: SqlitePool,
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
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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
        chat: chat_for_test,
    }
}

async fn req(
    app: &Router,
    sess: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn member_can_view_info_page() {
    let t = app().await;
    let (status, body) = req(&t.app, &t.member_session, Method::GET, "/room/1/info", "").await;
    assert_eq!(status, StatusCode::OK);
    // LC-454: the "Docs" tab was renamed "About"; Preferences was added.
    assert!(body.contains("About"), "tab nav present");
    assert!(body.contains("Preferences"), "preferences tab present");
    assert!(body.contains("Description"), "description section present");
    assert!(body.contains("Wiki"), "wiki section present");
    let _ = t.admin_id;
    let _ = t.member_id;
}

#[tokio::test]
async fn member_cannot_edit_wiki() {
    let t = app().await;
    let (status, _) = req(
        &t.app,
        &t.member_session,
        Method::PATCH,
        "/room/1/wiki",
        "body=hello",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_edit_wiki_and_audit_logged() {
    let t = app().await;
    let (status, body) = req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/wiki",
        "body=%23+hello+world",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // PATCH returns the rendered view fragment; markdown # heading should
    // render as <h1>.
    assert!(
        body.contains("<h1") && body.contains("hello world"),
        "wiki body rendered: {body}"
    );

    let stored: Option<String> = sqlx::query_scalar("SELECT wiki_body FROM rooms WHERE id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(stored.as_deref(), Some("# hello world"));

    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let row = actions
        .iter()
        .find(|a| a.action == "room_wiki_edit")
        .expect("wiki edit audit row");
    assert_eq!(row.actor_user, t.admin_id);
    assert_eq!(row.room_id, Some(1));
}

#[tokio::test]
async fn member_cannot_edit_description() {
    let t = app().await;
    let (status, _) = req(
        &t.app,
        &t.member_session,
        Method::PATCH,
        "/room/1/description",
        "body=hi",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_set_description() {
    let t = app().await;
    let (status, body) = req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/description",
        "body=room+for+announcements",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("room for announcements"), "rendered: {body}");
    let stored: Option<String> = sqlx::query_scalar("SELECT description FROM rooms WHERE id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(stored.as_deref(), Some("room for announcements"));
}

#[tokio::test]
async fn empty_wiki_body_clears_metadata() {
    let t = app().await;
    // First set a body.
    req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/wiki",
        "body=hello",
    )
    .await;
    // Then clear.
    let (status, _) = req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/wiki",
        "body=",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (body, at, by): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT wiki_body, wiki_updated_at, wiki_updated_by FROM rooms WHERE id=1")
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert!(body.is_none() && at.is_none() && by.is_none());
}

// LC-153-WIKI-DESC-CAP (#283): description + wiki bodies are capped like chat
// messages so a moderator cannot feed a multi-MiB body to the markdown
// renderer. MAX_DOC_CHARS is 64_000; the boundary is inclusive.
#[tokio::test]
async fn over_long_wiki_rejected() {
    let t = app().await;
    let body = format!("body={}", "a".repeat(64_001));
    let (status, _) = req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/wiki",
        &body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "64001-char wiki must be rejected"
    );
    // Nothing was written.
    let stored: Option<String> = sqlx::query_scalar("SELECT wiki_body FROM rooms WHERE id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert!(stored.is_none(), "rejected wiki must not be stored");
}

#[tokio::test]
async fn over_long_description_rejected() {
    let t = app().await;
    let body = format!("body={}", "a".repeat(64_001));
    let (status, _) = req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/description",
        &body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "64001-char description must be rejected"
    );
    let stored: Option<String> = sqlx::query_scalar("SELECT description FROM rooms WHERE id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert!(stored.is_none(), "rejected description must not be stored");
}

#[tokio::test]
async fn wiki_at_cap_accepted() {
    let t = app().await;
    let body = format!("body={}", "a".repeat(64_000));
    let (status, _) = req(
        &t.app,
        &t.admin_session,
        Method::PATCH,
        "/room/1/wiki",
        &body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "64000-char wiki is exactly at the cap"
    );
    let stored: Option<String> = sqlx::query_scalar("SELECT wiki_body FROM rooms WHERE id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(stored.as_deref().map(str::len), Some(64_000));
}
