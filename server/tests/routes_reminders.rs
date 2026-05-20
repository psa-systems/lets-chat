//! LC-63 integration: message reminders.
//!
//! Covers create (preset) + inline confirm, the management list, owner-
//! scoped cancel, picker room-access gating, and the dispatcher firing
//! (claims due rows, skips future + soft-deleted-message reminders).

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, reminders, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-reminders-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    state: AppState,
    alice: String,
    alice_session: String,
    bob_session: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    for id in [&alice, &bob] {
        sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
            .bind(id)
            .execute(&auth)
            .await
            .unwrap();
    }
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
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
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state.clone());
    TestApp {
        app,
        state,
        alice,
        alice_session,
        bob_session,
        chat,
    }
}

async fn send(
    app: &Router,
    sess: Option<&str>,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        b = b.header(header::COOKIE, format!("session={s}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Public room + one message; returns (room_id, message_id).
async fn seed_public_message(t: &TestApp) -> (i64, i64) {
    // Public room inside an enclave Alice owns: is_room_accessible grants
    // public rooms to every enclave member.
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.alice)
        .await
        .unwrap();
    let room = db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap();
    let mid = db::chat::insert_message(&t.chat, room, &t.alice, "remember the milk")
        .await
        .unwrap();
    (room, mid)
}

#[tokio::test]
async fn create_preset_then_list_and_cancel() {
    let t = app().await;
    let (_room, mid) = seed_public_message(&t).await;

    let (status, body) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        "/reminders",
        &format!("message_id={mid}&preset=1h"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Reminder set"),
        "inline confirmation rendered"
    );

    let pending = db::reminders::list_pending_for_user(&t.chat, &t.alice)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "one pending reminder");
    let id = pending[0].id;

    let (status, page) = send(
        &t.app,
        Some(&t.alice_session),
        Method::GET,
        "/reminders",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("remember the milk"), "snippet shown on page");

    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::DELETE,
        &format!("/reminders/{id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after = db::reminders::list_pending_for_user(&t.chat, &t.alice)
        .await
        .unwrap();
    assert!(after.is_empty(), "cancel removed the reminder");
}

#[tokio::test]
async fn cancel_is_owner_scoped() {
    let t = app().await;
    let (_room, mid) = seed_public_message(&t).await;
    let id = db::reminders::insert(&t.chat, &t.alice, mid, "2999-01-01 00:00:00")
        .await
        .unwrap();
    // Bob cannot cancel Alice's reminder.
    let (status, _) = send(
        &t.app,
        Some(&t.bob_session),
        Method::DELETE,
        &format!("/reminders/{id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        db::reminders::list_pending_for_user(&t.chat, &t.alice)
            .await
            .unwrap()
            .len(),
        1,
        "Alice's reminder survives Bob's cancel attempt"
    );
}

#[tokio::test]
async fn picker_requires_room_access() {
    let t = app().await;
    // Private room in Alice's enclave; Bob is not an enclave member.
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.alice)
        .await
        .unwrap();
    let room = db::chat::create_room(&t.chat, "secret", None, "private", Some("code"), Some(eid))
        .await
        .unwrap();
    db::chat::add_room_member(&t.chat, room, &t.alice)
        .await
        .unwrap();
    let mid = db::chat::insert_message(&t.chat, room, &t.alice, "psst")
        .await
        .unwrap();

    let (status, _) = send(
        &t.app,
        Some(&t.bob_session),
        Method::GET,
        &format!("/reminders/picker?message_id={mid}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "outsider blocked");

    let (status, body) = send(
        &t.app,
        Some(&t.alice_session),
        Method::GET,
        &format!("/reminders/picker?message_id={mid}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Remind me about this"),
        "picker rendered for member"
    );
}

#[tokio::test]
async fn dispatcher_fires_due_skips_future() {
    let t = app().await;
    let (_room, mid) = seed_public_message(&t).await;
    // One due (past), one not-yet (future).
    let due = db::reminders::insert(&t.chat, &t.alice, mid, "2000-01-01 00:00:00")
        .await
        .unwrap();
    let future = db::reminders::insert(&t.chat, &t.alice, mid, "2999-01-01 00:00:00")
        .await
        .unwrap();

    let stats = reminders::run_reminder_tick(&t.state).await.unwrap();
    assert_eq!(stats.fired, 1, "only the due reminder fires");

    // Due is now claimed (no longer pending); future still pending.
    let pending = db::reminders::list_pending_for_user(&t.chat, &t.alice)
        .await
        .unwrap();
    let ids: Vec<i64> = pending.iter().map(|r| r.id).collect();
    assert!(!ids.contains(&due), "fired reminder left the pending list");
    assert!(ids.contains(&future), "future reminder still pending");

    // A second tick fires nothing (already claimed).
    let again = reminders::run_reminder_tick(&t.state).await.unwrap();
    assert_eq!(again.fired, 0, "no double-fire");
}

#[tokio::test]
async fn soft_deleted_message_reminder_does_not_fire() {
    let t = app().await;
    let (_room, mid) = seed_public_message(&t).await;
    db::reminders::insert(&t.chat, &t.alice, mid, "2000-01-01 00:00:00")
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(mid)
        .execute(&t.chat)
        .await
        .unwrap();

    let stats = reminders::run_reminder_tick(&t.state).await.unwrap();
    assert_eq!(
        stats.fired, 0,
        "reminder for a deleted message does not fire"
    );
}
