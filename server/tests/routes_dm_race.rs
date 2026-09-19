//! LC-950 Part 4: `GET /dm/{peer_id}` must adopt `db::chat::find_or_create_dm_room`
//! (LC-909's race-safe helper) instead of hand-rolling its own find-then-create,
//! and must skip the "new DM" sidebar nudge on the side of a request that lost
//! the create race (found an already-existing room rather than creating one).
//!
//! This drives the race scenario end to end through the route (not just the
//! helper in isolation): seed the DM room out of band first - simulating a
//! concurrent request that already won the create - then hit the route and
//! assert its own RoomMemberAdded nudge never fires.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes, state::AppState};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast::error::TryRecvError;
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-dm-race-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    hub: Arc<Hub>,
    session: String,
    viewer_id: String,
    peer_id: String,
    chat: sqlx::SqlitePool,
}

async fn app_with_two_users(viewer: &str, peer: &str) -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, viewer, "hash").await.unwrap();
    let peer_id = db::auth::create_user(&auth, peer, "hash").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id IN (?, ?)")
        .bind(&viewer_id)
        .bind(&peer_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let hub = Arc::new(Hub::new());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: hub.clone(),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
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
        hub,
        session,
        viewer_id,
        peer_id,
        chat: chat_for_test,
    }
}

async fn get_dm_page(app: &Router, sess: &str, peer_id: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/dm/{peer_id}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    resp.status()
}

async fn count_dm_rooms(chat: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM rooms WHERE room_type = 'dm'")
        .fetch_one(chat)
        .await
        .unwrap()
}

/// The losing side of the create race: the room already exists (created out
/// of band, standing in for a concurrent winning request) before the route
/// runs, so `find_or_create_dm_room` must report `created = false` and the
/// route must not fire its RoomMemberAdded nudge.
#[tokio::test]
async fn get_dm_does_not_nudge_when_the_room_already_exists() {
    let t = app_with_two_users("viewer", "alice").await;
    db::chat::create_dm_room(&t.chat, "@alice", &t.viewer_id, &t.peer_id)
        .await
        .unwrap();
    assert_eq!(count_dm_rooms(&t.chat).await, 1);

    // Connect both parties so a nudge, if fired, would land in these queues.
    let (_conn, mut viewer_rx, _) = t.hub.connect(&t.viewer_id, "viewer");
    let (_conn2, mut peer_rx, _) = t.hub.connect(&t.peer_id, "alice");

    let status = get_dm_page(&t.app, &t.session, &t.peer_id).await;
    assert_eq!(status, StatusCode::OK);

    // No second room was created, and no RoomMemberAdded nudge was sent to
    // either party: this request found the room, it did not create it.
    assert_eq!(count_dm_rooms(&t.chat).await, 1);
    assert!(
        matches!(viewer_rx.try_recv(), Err(TryRecvError::Empty)),
        "viewer must not get a 'new DM' nudge for a room that already existed"
    );
    assert!(
        matches!(peer_rx.try_recv(), Err(TryRecvError::Empty)),
        "peer must not get a 'new DM' nudge for a room that already existed"
    );
}

/// The winning side: no room exists yet, so the route creates it (through
/// `find_or_create_dm_room`) and both parties DO get the sidebar nudge.
#[tokio::test]
async fn get_dm_nudges_both_parties_when_it_creates_the_room() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(count_dm_rooms(&t.chat).await, 0);

    let (_conn, mut viewer_rx, _) = t.hub.connect(&t.viewer_id, "viewer");
    let (_conn2, mut peer_rx, _) = t.hub.connect(&t.peer_id, "alice");

    let status = get_dm_page(&t.app, &t.session, &t.peer_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count_dm_rooms(&t.chat).await, 1);

    assert!(
        matches!(viewer_rx.try_recv(), Ok(ChatEvent::RoomMemberAdded { .. })),
        "viewer must get the 'new DM' nudge when this request creates the room"
    );
    assert!(
        matches!(peer_rx.try_recv(), Ok(ChatEvent::RoomMemberAdded { .. })),
        "peer must get the 'new DM' nudge when this request creates the room"
    );
}
