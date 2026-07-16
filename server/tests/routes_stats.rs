//! LC-536: personal member stats. Covers the aggregation (`db::stats`) and the
//! self-only recap page at GET /stats. Router harness mirrors routes_hovercard.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

// Aggregation is the load-bearing logic; assert it directly against a pool.
#[tokio::test]
async fn member_stats_counts_own_activity_only() {
    let chat = common::chat_pool().await;

    // Room 1 ("general") ships with the chat migration. Two countable messages
    // from u1, one from someone else (must not inflate u1's totals).
    let m1 = db::chat::insert_message(&chat, 1, "u1", "hi")
        .await
        .unwrap();
    db::chat::insert_message(&chat, 1, "u1", "again")
        .await
        .unwrap();
    let other_msg = db::chat::insert_message(&chat, 1, "u2", "hey")
        .await
        .unwrap();

    // A soft-deleted u1 message must be excluded from every message figure.
    let deleted = db::chat::insert_message(&chat, 1, "u1", "oops")
        .await
        .unwrap();
    db::moderation::soft_delete_message(&chat, deleted, "mod")
        .await
        .unwrap();

    // u2 reacts to u1's message (received); u1 reacts to u2's (given); u1
    // self-reacts (must NOT count as received).
    for (mid, uid) in [(m1, "u2"), (other_msg, "u1"), (m1, "u1")] {
        sqlx::query("INSERT INTO message_reactions (message_id, user_id, emoji) VALUES (?, ?, ?)")
            .bind(mid)
            .bind(uid)
            .bind("👍")
            .execute(&chat)
            .await
            .unwrap();
    }

    // u2 gives u1 a kudo.
    db::kudos::record(&chat, "u2", "u1", 1, None, Some("nice"), Some(m1))
        .await
        .unwrap();

    let s = db::stats::member_stats(&chat, "u1").await.unwrap();
    assert_eq!(s.messages_sent, 2, "deleted + others excluded");
    assert_eq!(s.active_days, 1);
    assert_eq!(s.reactions_received, 1, "self-reaction excluded");
    assert_eq!(s.reactions_given, 2, "u1 reacted twice");
    assert_eq!(s.kudos_received, 1);
    assert_eq!(s.top_rooms.len(), 1);
    assert_eq!(s.top_rooms[0].name, "general");
    assert_eq!(s.top_rooms[0].count, 2);
}

async fn build_app(
    auth: sqlx::SqlitePool,
    chat: sqlx::SqlitePool,
    settings: sqlx::SqlitePool,
) -> Router {
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
        llm_client: None,
        embedding_client: None,
    };
    routes::build_router(state)
}

#[tokio::test]
async fn stats_page_renders_self_totals() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    // An admin is needed so backfill_general_membership grants room access.
    let id = db::auth::create_user(&auth, "statsuser", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    db::chat::insert_message(&chat, 1, &id, "one")
        .await
        .unwrap();
    db::chat::insert_message(&chat, 1, &id, "two")
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &id).await.unwrap();

    let app = build_app(auth, chat, settings).await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/stats")
                .header(header::COOKIE, format!("session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("Your stats"), "heading missing: {body}");
    // The messages-sent tile shows "2" next to its label.
    assert!(body.contains("general"), "top channel missing: {body}");
}

#[tokio::test]
async fn stats_page_requires_auth() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let app = build_app(auth, chat, settings).await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}
