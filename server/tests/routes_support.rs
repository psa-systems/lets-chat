//! LC-714/716: AI help desk support-ticket flow through the HTTP surface.
//! - `/human` with no admin available files a ticket into the queue.
//! - Resolving a ticket in the admin queue notifies the requester by a bot DM.
//! - Claiming a ticket opens a dedicated support channel joining the requester,
//!   the admin, and the assistant bot, and is a no-op if already claimed.

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

#[tokio::test]
async fn claiming_a_ticket_opens_a_shared_channel() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let admin2 = db::auth::create_user(&auth, "admin2", "h").await.unwrap();
    for a in [&admin, &admin2] {
        sqlx::query("UPDATE users SET role='admin' WHERE id=?")
            .bind(a)
            .execute(&auth)
            .await
            .unwrap();
    }
    let requester = db::auth::create_user(&auth, "member", "h").await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();
    let admin2_session = db::auth::create_session(&auth, &admin2).await.unwrap();

    let ticket_id = db::support_tickets::create(&chat, &requester, Some(1), "general", "help me")
        .await
        .unwrap();

    let app: Router =
        routes::build_router(state_from(auth.clone(), chat.clone(), settings.clone()));

    let claim = |session: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri(format!("/admin/support/{ticket_id}/claim"))
                .header(header::COOKIE, format!("session={session}"))
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap()
        }
    };

    let res = claim(admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Ticket left the open queue (claimed, not resolved).
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 0);

    // A dedicated private support room was created, joining requester + admin + bot.
    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created");
    let (room_id, room_name, room_type): (i64, String, String) =
        sqlx::query_as("SELECT id, name, room_type FROM rooms WHERE name LIKE 'Support:%'")
            .fetch_one(&chat)
            .await
            .expect("support room created");
    assert_eq!(room_type, "private", "support room is private");
    // LC-725: the claim response redirects the acting admin into that new channel.
    assert_eq!(
        res.headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some(format!("/room/{room_id}").as_str()),
        "claim redirects the admin to the support channel"
    );
    assert!(
        room_name.contains(&format!("#{ticket_id}")),
        "room name carries the ticket id, got: {room_name}"
    );
    let members: Vec<String> =
        sqlx::query_scalar("SELECT user_id FROM room_members WHERE room_id=?")
            .bind(room_id)
            .fetch_all(&chat)
            .await
            .unwrap();
    for expected in [&requester, &admin, &bot.id] {
        assert!(
            members.contains(expected),
            "support room includes {expected}, members: {members:?}"
        );
    }

    // The requester got a bot DM pointing at the new channel.
    let dm = db::chat::find_dm_room(&chat, &bot.id, &requester)
        .await
        .unwrap()
        .expect("bot DM to the requester exists");
    let dm_body: String =
        sqlx::query_scalar("SELECT body FROM messages WHERE room_id=? ORDER BY id DESC LIMIT 1")
            .bind(dm.id)
            .fetch_one(&chat)
            .await
            .unwrap();
    assert!(
        dm_body.contains(&format!("/room/{room_id}")),
        "DM links to the support channel, got: {dm_body}"
    );

    // A second admin claiming the same ticket is a no-op: no second support room.
    let res2 = claim(admin2_session).await;
    assert_eq!(res2.status(), StatusCode::OK);
    let support_rooms: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rooms WHERE name LIKE 'Support:%'")
            .fetch_one(&chat)
            .await
            .unwrap();
    assert_eq!(support_rooms, 1, "claim is idempotent; no duplicate room");
}

// LC-726: pending support requests surface outside the admin section - an
// admin-only rail tile (every page) and a Home dashboard card - so an admin does
// not have to be in /admin to see the queue. A non-admin sees neither.
#[tokio::test]
async fn pending_support_surfaces_on_home_for_admins_only() {
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
    let member_session = db::auth::create_session(&auth, &requester).await.unwrap();
    db::support_tickets::create(&chat, &requester, Some(1), "general", "printer on fire")
        .await
        .unwrap();

    let app: Router = routes::build_router(state_from(auth.clone(), chat.clone(), settings));

    let home = |session: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .method(Method::GET)
                .uri("/?home=1")
                .header(header::COOKIE, format!("session={session}"))
                .body(Body::empty())
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        }
    };

    // Admin: rail Support tile + dashboard card with the pending request.
    let admin_home = home(admin_session).await;
    assert!(
        admin_home.contains("lc-rail-support"),
        "admin rail shows the Support tile"
    );
    assert!(
        admin_home.contains("lc-home-support-card") && admin_home.contains("printer on fire"),
        "admin dashboard shows the pending request card"
    );

    // Non-admin: neither surface.
    let member_home = home(member_session).await;
    assert!(
        !member_home.contains("lc-rail-support"),
        "a non-admin never sees the Support rail tile"
    );
    assert!(
        !member_home.contains("printer on fire"),
        "a non-admin never sees pending support content"
    );
}
