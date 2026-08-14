//! LC-714: AI help desk support-ticket flow through the HTTP surface.
//! - `/human` with no admin available files a ticket into the queue.
//! - Resolving a ticket in the admin queue notifies the requester by a bot DM.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes};
use tower::ServiceExt;

mod common;

fn state_from(
    auth: sqlx::SqlitePool,
    chat: sqlx::SqlitePool,
    settings: sqlx::SqlitePool,
) -> AppState {
    let bg = lets_chat::bg::spawn(auth.clone());
    AppState {
        geoip: None,
        login_approval_enabled: false,
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
        llm_client: Some(Arc::new(lets_chat::llm::MockLlmClient {
            canned: "unused".into(),
        })),
        embedding_client: None,
    }
}

async fn enable_ai(settings: &sqlx::SqlitePool) {
    db::settings::set_setting(settings, "llm_enabled", "true")
        .await
        .unwrap();
}

#[tokio::test]
async fn human_with_no_admin_files_a_ticket() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    // An admin (so General membership backfills and the member can post) plus a
    // member. The admin is made idle (activity older than the availability
    // window), so no admin is "available now" and the escalation falls through to
    // filing a ticket.
    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let member = db::auth::create_user(&auth, "member", "h").await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET last_active_at = datetime('now','-1 day'), last_ws_seen_at = NULL WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &member).await.unwrap();
    enable_ai(&settings).await;

    let app: Router = routes::build_router(state_from(auth, chat.clone(), settings));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/messages")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=/human+account+locked"))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // A ticket was filed capturing the request, from the origin room.
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 1);
    let open = db::support_tickets::list_open(&chat).await.unwrap();
    assert_eq!(open[0].requester_id, member);
    assert_eq!(open[0].room_id, Some(1));
    assert!(
        open[0].body.contains("account locked"),
        "ticket carries the request, got: {}",
        open[0].body
    );
}

#[tokio::test]
async fn resolving_a_ticket_notifies_the_requester() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let requester = db::auth::create_user(&auth, "member", "h").await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();

    // A ticket already in the queue.
    let ticket_id = db::support_tickets::create(&chat, &requester, Some(1), "general", "help me")
        .await
        .unwrap();

    let app: Router = routes::build_router(state_from(auth.clone(), chat.clone(), settings));

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/admin/support/{ticket_id}/resolve"))
        .header(header::COOKIE, format!("session={admin_session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The ticket is resolved...
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 0);

    // ...and the requester got a bot DM about it.
    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created for the notification");
    let dm = db::chat::find_dm_room(&chat, &bot.id, &requester)
        .await
        .unwrap()
        .expect("bot DM to the requester exists");
    let body: String =
        sqlx::query_scalar("SELECT body FROM messages WHERE room_id=? ORDER BY id DESC LIMIT 1")
            .bind(dm.id)
            .fetch_one(&chat)
            .await
            .unwrap();
    assert!(
        body.contains("resolved"),
        "requester told it was resolved, got: {body}"
    );
    assert!(
        body.contains(&format!("#{ticket_id}")),
        "DM references the ticket number, got: {body}"
    );
}
