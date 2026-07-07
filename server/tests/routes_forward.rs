//! LC-278: message forwarding. A message can be reposted into another room the
//! viewer can post to, gated end to end; the picker lists destinations.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-forward-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

struct TestApp {
    app: Router,
    chat: SqlitePool,
    admin_id: String,
    member_session: String,
}

async fn setup() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: None,
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
        admin_id,
        member_session,
    }
}

async fn public_room(t: &TestApp, name: &str) -> i64 {
    let g: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, name, None, "public", None, Some(g))
        .await
        .unwrap()
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn forward_reposts_into_destination_room() {
    let t = setup().await;
    // A General (room 1) message authored by admin; member forwards it.
    let msg = db::chat::insert_message(&t.chat, 1, &t.admin_id, "the original text")
        .await
        .unwrap();
    let dest = public_room(&t, "destination").await;

    let (status, _) = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/messages/{msg}/forward/{dest}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The destination room now holds a forwarded message carrying the text.
    let msgs = db::chat::list_messages(&t.chat, dest).await.unwrap();
    assert!(
        msgs.iter().any(|m| m.body.contains("the original text")),
        "forwarded message must appear in the destination with the original text",
    );
    assert!(
        msgs.iter().any(|m| m.body.contains("Forwarded from")),
        "forwarded message must carry the attribution header",
    );
}

#[tokio::test]
async fn forward_forbidden_into_inaccessible_room() {
    let t = setup().await;
    let msg = db::chat::insert_message(&t.chat, 1, &t.admin_id, "secret-worthy")
        .await
        .unwrap();
    // A private room with no members; member (non-admin) cannot post there.
    let priv_room = db::chat::create_room(&t.chat, "secret", None, "private", None, None)
        .await
        .unwrap();

    let (status, _) = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/messages/{msg}/forward/{priv_room}"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn forward_picker_lists_destinations_excluding_source() {
    let t = setup().await;
    let msg = db::chat::insert_message(&t.chat, 1, &t.admin_id, "hi")
        .await
        .unwrap();
    let dest = public_room(&t, "elsewhere").await;

    let (status, body) = send(
        &t.app,
        &t.member_session,
        Method::GET,
        &format!("/messages/{msg}/forward"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-forward-dialog"),
        "picker must render the modal: {body}"
    );
    assert!(
        body.contains(&format!("/forward/{dest}\"")),
        "picker must offer the destination room: {body}"
    );
    assert!(
        !body.contains("/forward/1\""),
        "picker must exclude the source room (General, id 1): {body}"
    );
}
