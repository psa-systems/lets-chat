//! LC-941: the room's own AI toggle (`rooms.assistant_enabled`) must govern
//! every per-room AI surface, not just `/ask`. Covers, with the global flag ON
//! and the default "everyone" audience (so only the room toggle is under
//! test): translate, suggest-reply, compose-assist, find-related, and the
//! per-room summary all refuse with the room toggle off; semantic search
//! falls back to keyword ranking; and a message sent (or backfilled) in a
//! toggled-off room gets no embeddings row. Harness mirrors routes_ai_gate.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::embeddings::{hash_embed, vec_to_bytes, MockEmbeddingClient};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct Setup {
    app: Router,
    state: AppState,
    member_session: String,
    member_id: String,
    room_id: i64,
}

async fn setup() -> Setup {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let member = db::auth::create_user(&auth, "member", "h").await.unwrap();
    // backfill_general_membership requires at least one admin user to run, and
    // that's what enrolls both users in the General room/enclave (mirrors
    // routes_ai_gate.rs's Setup).
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let member_session = db::auth::create_session(&auth, &member).await.unwrap();

    // LC-679: opt this AI-feature test into the runtime flag + default
    // "everyone" audience, so the only variable under test is the room toggle.
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
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
        llm_client: Some(Arc::new(lets_chat::llm::MockLlmClient {
            canned: "Polished draft.".into(),
        })),
        embedding_client: Some(Arc::new(MockEmbeddingClient::default())),
    };

    // Room 1 is General; the member is already in it via the backfill.
    let room_id = 1;
    Setup {
        app: routes::build_router(state.clone()),
        state,
        member_session,
        member_id: member,
        room_id,
    }
}

async fn set_room_toggle(chat: &sqlx::SqlitePool, room_id: i64, on: bool) {
    db::chat::set_room_assistant_enabled(chat, room_id, on)
        .await
        .unwrap();
}

async fn req(app: &Router, method: Method, uri: &str, session: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn get_body(app: &Router, uri: &str, session: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn room_toggle_off_refuses_every_ai_route_guard() {
    let s = setup().await;
    let msg_id = db::chat::insert_message(&s.state.chat, s.room_id, &s.member_id, "hello there")
        .await
        .unwrap();
    set_room_toggle(&s.state.chat, s.room_id, false).await;

    assert_eq!(
        req(
            &s.app,
            Method::POST,
            &format!("/messages/{msg_id}/translate"),
            &s.member_session,
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
        "translate must refuse when the room's AI toggle is off"
    );

    assert_eq!(
        req(
            &s.app,
            Method::POST,
            &format!("/messages/{msg_id}/suggest-reply"),
            &s.member_session,
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
        "suggest-reply must refuse when the room's AI toggle is off"
    );

    assert_eq!(
        req(
            &s.app,
            Method::POST,
            &format!("/room/{}/compose-assist", s.room_id),
            &s.member_session,
            "action=grammar&body=helo+world",
        )
        .await,
        StatusCode::FORBIDDEN,
        "compose-assist must refuse when the room's AI toggle is off"
    );

    assert_eq!(
        req(
            &s.app,
            Method::GET,
            &format!("/messages/{msg_id}/related"),
            &s.member_session,
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
        "find-related must refuse when the room's AI toggle is off"
    );

    assert_eq!(
        req(
            &s.app,
            Method::POST,
            &format!("/room/{}/summary", s.room_id),
            &s.member_session,
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
        "the per-room summary must refuse when the room's AI toggle is off"
    );
}

#[tokio::test]
async fn room_toggle_on_allows_the_same_routes() {
    let s = setup().await;
    let msg_id = db::chat::insert_message(&s.state.chat, s.room_id, &s.member_id, "hello there")
        .await
        .unwrap();
    set_room_toggle(&s.state.chat, s.room_id, true).await;

    assert_eq!(
        req(
            &s.app,
            Method::POST,
            &format!("/messages/{msg_id}/translate"),
            &s.member_session,
            "",
        )
        .await,
        StatusCode::OK,
        "translate must be allowed once the room's AI toggle is on"
    );
}

/// Semantic ranking only considers messages that already have a stored
/// embedding row (`message_embeddings::list_for_room`); FTS keyword search
/// considers every message regardless. So embedding exactly one of two
/// same-keyword messages discriminates the two ranking modes: semantic mode
/// surfaces only the embedded one, FTS surfaces both.
#[tokio::test]
async fn room_toggle_off_falls_back_to_keyword_semantic_search() {
    let s = setup().await;

    let embedded_id = db::chat::insert_message(
        &s.state.chat,
        s.room_id,
        &s.member_id,
        "gizmo release notes",
    )
    .await
    .unwrap();
    let vec = hash_embed("gizmo release notes", MockEmbeddingClient::default().dim);
    db::message_embeddings::upsert(
        &s.state.chat,
        embedded_id,
        s.room_id,
        &MockEmbeddingClient::default().model,
        vec.len() as i64,
        &vec_to_bytes(&vec),
    )
    .await
    .unwrap();
    db::chat::insert_message(
        &s.state.chat,
        s.room_id,
        &s.member_id,
        "gizmo shipping delay",
    )
    .await
    .unwrap();

    let uri = format!("/search?room_id={}&q=gizmo&semantic=1", s.room_id);

    set_room_toggle(&s.state.chat, s.room_id, false).await;
    let body_off = get_body(&s.app, &uri, &s.member_session).await;
    assert!(
        body_off.contains("release notes") && body_off.contains("shipping delay"),
        "room toggle off must fall back to keyword search over every match: {body_off}"
    );

    set_room_toggle(&s.state.chat, s.room_id, true).await;
    let body_on = get_body(&s.app, &uri, &s.member_session).await;
    assert!(
        body_on.contains("release notes"),
        "room toggle on: the embedded message should still rank: {body_on}"
    );
    assert!(
        !body_on.contains("shipping delay"),
        "room toggle on: semantic ranking should only surface the embedded message: {body_on}"
    );
}

#[tokio::test]
async fn room_toggle_off_excludes_messages_from_the_embeddings_index() {
    let s = setup().await;
    set_room_toggle(&s.state.chat, s.room_id, false).await;

    let msg_id = db::chat::insert_message(&s.state.chat, s.room_id, &s.member_id, "secret plans")
        .await
        .unwrap();

    // The send-path spawns embed_message in the background; drive the same
    // logic synchronously via the backfill dispatcher, which also calls
    // embed_message per pending row.
    lets_chat::routes::related::run_embedding_backfill_tick(&s.state, 50)
        .await
        .unwrap();
    assert!(
        !db::message_embeddings::exists(&s.state.chat, msg_id)
            .await
            .unwrap(),
        "a message posted to a toggled-off room must have no embeddings row after backfill"
    );

    // Flip the room back on: the backfill drains the same still-pending message.
    set_room_toggle(&s.state.chat, s.room_id, true).await;
    let n2 = lets_chat::routes::related::run_embedding_backfill_tick(&s.state, 50)
        .await
        .unwrap();
    assert_eq!(
        n2, 1,
        "the message becomes eligible once the room is opted back in"
    );
    assert!(db::message_embeddings::exists(&s.state.chat, msg_id)
        .await
        .unwrap());
}
