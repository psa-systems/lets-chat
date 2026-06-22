//! LC-260: quick switcher (`GET /switcher`).
//!
//! Returns an access-correct, flat list of rooms (current enclave) + the
//! viewer's DMs + people, as a listbox fragment for the Ctrl/Cmd+K palette.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    chat: SqlitePool,
    alice_id: String,
    alice_session: String,
    bob_id: String,
    bob_session: String,
    carol_id: String,
    zonk_id: String,
    enclave_id: i64,
}

async fn setup() -> TestApp {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    let carol_id = db::auth::create_user(&auth, "carol", "h").await.unwrap();
    let zonk_id = db::auth::create_user(&auth, "zonk", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice_id).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // The General room (id 1) anchors the General enclave; reuse its enclave.
    let enclave_id: i64 = sqlx::query("SELECT enclave_id FROM rooms WHERE id = 1")
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("enclave_id");

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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        chat,
        alice_id,
        alice_session,
        bob_id,
        bob_session,
        carol_id,
        zonk_id,
        enclave_id,
    }
}

async fn new_room(chat: &SqlitePool, name: &str, room_type: &str, enclave_id: i64) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type, enclave_id) VALUES (?, ?, ?)")
        .bind(name)
        .bind(room_type)
        .bind(enclave_id)
        .execute(chat)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn switcher(app: &Router, sess: &str, q: &str, enclave_id: i64) -> (StatusCode, String) {
    let uri = format!("/switcher?q={q}&enclave_id={enclave_id}");
    let req = Request::builder()
        .method(Method::GET)
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
async fn switcher_matches_accessible_room() {
    let t = setup().await;
    let rid = new_room(&t.chat, "alpha-room", "public", t.enclave_id).await;

    let (status, body) = switcher(&t.app, &t.alice_session, "alpha", t.enclave_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("href=\"/room/{rid}\"")),
        "matching room must appear as a /room/{{id}} option",
    );
    assert!(body.contains("alpha-room"), "room label rendered");
}

#[tokio::test]
async fn switcher_matches_dm_peer() {
    let t = setup().await;
    db::chat::create_dm_room(&t.chat, "@bob", &t.alice_id, &t.bob_id)
        .await
        .unwrap();

    let (status, body) = switcher(&t.app, &t.alice_session, "bob", t.enclave_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("href=\"/dm/{}\"", t.bob_id)),
        "a DM peer matching the query is a /dm/{{id}} option",
    );
}

#[tokio::test]
async fn switcher_person_not_duplicated() {
    let t = setup().await;
    // carol has no DM with alice: she appears once, sourced from people search.
    let (status, body) = switcher(&t.app, &t.alice_session, "carol", t.enclave_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.matches(&format!("href=\"/dm/{}\"", t.carol_id))
            .count(),
        1,
        "a person appears exactly once",
    );
}

#[tokio::test]
async fn switcher_empty_query_excludes_people() {
    let t = setup().await;
    let (status, body) = switcher(&t.app, &t.alice_session, "", t.enclave_id).await;
    assert_eq!(status, StatusCode::OK);
    // zonk is a public user with no DM; an empty query lists rooms + DMs only.
    assert!(
        !body.contains(&format!("href=\"/dm/{}\"", t.zonk_id)),
        "people are not surfaced for an empty query",
    );
}

#[tokio::test]
async fn switcher_hides_inaccessible_room() {
    let t = setup().await;
    let secret = new_room(&t.chat, "secret-room", "private", t.enclave_id).await;

    // bob is a non-admin and not a member of the private room.
    let (status, body) = switcher(&t.app, &t.bob_session, "secret", t.enclave_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains(&format!("href=\"/room/{secret}\"")),
        "a room the viewer cannot access must not appear",
    );
}
