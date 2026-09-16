//! LC-920: refuse to take a follow-up item someone else holds.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, models::enclave::EnclaveRole, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-followups-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    alice: String,
    alice_session: String,
    bob: String,
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
        geoip: None,
        login_approval_enabled: false,
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
        alice,
        alice_session,
        bob,
        bob_session,
        chat,
    }
}

async fn send(app: &Router, sess: Option<&str>, method: Method, uri: &str) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(s) = sess {
        b = b.header(header::COOKIE, format!("session={s}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Public room in an enclave Alice owns, with both Alice and Bob as members.
async fn seed_room(t: &TestApp) -> i64 {
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.alice)
        .await
        .unwrap();
    db::enclave::add_member(&t.chat, eid, &t.bob, EnclaveRole::Member)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap()
}

#[tokio::test]
async fn claim_conflict_names_the_holder_and_leaves_assignment_untouched() {
    let t = app().await;
    let room = seed_room(&t).await;
    let mid = db::followups::create(&t.chat, room, &t.alice, "t", None, &["Ship it".into()])
        .await
        .unwrap();
    let id = db::followups::items(&t.chat, mid).await.unwrap()[0].id;

    // Alice claims it.
    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/follow-up/{id}/claim"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        db::followups::item(&t.chat, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some(t.alice.as_str())
    );

    // Bob tries to take it from Alice: refused, Alice still holds it.
    let (status, body) = send(
        &t.app,
        Some(&t.bob_session),
        Method::POST,
        &format!("/follow-up/{id}/claim"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "taking another user's item is 409: {body}"
    );
    assert_eq!(
        db::followups::item(&t.chat, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some(t.alice.as_str()),
        "assignee unchanged after the refused claim"
    );

    // Alice can still release her own item.
    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/follow-up/{id}/claim"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(db::followups::item(&t.chat, id)
        .await
        .unwrap()
        .unwrap()
        .assignee_id
        .is_none());

    // Now unclaimed, Bob can claim it.
    let (status, _) = send(
        &t.app,
        Some(&t.bob_session),
        Method::POST,
        &format!("/follow-up/{id}/claim"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        db::followups::item(&t.chat, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some(t.bob.as_str())
    );
}
