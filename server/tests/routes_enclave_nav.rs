//! LC-143: clicking an enclave opens the last-/default room; settings via a
//! gear (manager-only). Covers the redirect, last-room persistence, the
//! no-rooms landing fallback, and the gear's RBAC.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-enclnav-tests-{}", std::process::id()));
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

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, Option<String>, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let loc = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, loc, String::from_utf8_lossy(&bytes).into_owned())
}

/// Enclave owned by Alice (Bob a plain member), with two rooms.
/// Returns (enclave_id, alpha_room, beta_room).
async fn seed(t: &TestApp) -> (i64, i64, i64) {
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.alice)
        .await
        .unwrap();
    db::enclave::add_member(&t.chat, eid, &t.bob, EnclaveRole::Member)
        .await
        .unwrap();
    let alpha = db::chat::create_room(&t.chat, "alpha", None, "public", None, Some(eid))
        .await
        .unwrap();
    let beta = db::chat::create_room(&t.chat, "beta", None, "public", None, Some(eid))
        .await
        .unwrap();
    (eid, alpha, beta)
}

#[tokio::test]
async fn enclave_click_redirects_to_default_room() {
    let t = app().await;
    let (eid, alpha, _beta) = seed(&t).await;
    let (status, loc, _) = get(&t.app, &t.alice_session, &format!("/enclave/{eid}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        loc.as_deref(),
        Some(format!("/room/{alpha}").as_str()),
        "first room by name is the default"
    );
}

#[tokio::test]
async fn enclave_click_reopens_last_room() {
    let t = app().await;
    let (eid, _alpha, beta) = seed(&t).await;
    // Open beta -> records it as the last room for this enclave.
    let (s, _, _) = get(&t.app, &t.alice_session, &format!("/room/{beta}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        db::enclave::get_last_room(&t.chat, &t.alice, eid)
            .await
            .unwrap(),
        Some(beta),
        "room open recorded"
    );
    let (status, loc, _) = get(&t.app, &t.alice_session, &format!("/enclave/{eid}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(loc.as_deref(), Some(format!("/room/{beta}").as_str()));
}

#[tokio::test]
async fn empty_enclave_renders_landing() {
    let t = app().await;
    let eid = db::enclave::create_enclave(&t.chat, "Empty", None, &t.alice)
        .await
        .unwrap();
    let (status, _, body) = get(&t.app, &t.alice_session, &format!("/enclave/{eid}")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "no rooms -> landing page, not a redirect"
    );
    assert!(body.contains("Empty"), "renders the enclave landing");
}

#[tokio::test]
async fn settings_gear_visible_to_manager_only() {
    let t = app().await;
    let (_eid, alpha, _beta) = seed(&t).await;
    // Owner sees the gear in the switcher rail.
    let (_, _, owner_body) = get(&t.app, &t.alice_session, &format!("/room/{alpha}")).await;
    assert!(
        owner_body.contains("/branding/logo") || owner_body.contains("Enclave settings"),
        "owner page renders"
    );
    assert!(
        owner_body.contains("Enclave settings"),
        "owner sees the settings gear"
    );
    // Plain member does not.
    let (_, _, member_body) = get(&t.app, &t.bob_session, &format!("/room/{alpha}")).await;
    assert!(
        !member_body.contains("Enclave settings"),
        "plain member does not see the settings gear"
    );
}

// LC-174: a room page in an enclave subscribes to that enclave's topic and its
// sidebar nav carries the enclave-keyed id, so a room add/remove in the enclave
// OOB-swaps the nav for members sitting in one of its rooms (not just on the
// landing). The enclave-keyed id is what makes the swap land only on viewers of
// THIS enclave; Home / another enclave have a different id and drop it.
#[tokio::test]
async fn room_page_subscribes_to_enclave_topic_and_keys_sidebar_nav() {
    let t = app().await;
    let (eid, alpha, _beta) = seed(&t).await;
    let (status, _, body) = get(&t.app, &t.alice_session, &format!("/room/{alpha}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("data-lc-live-topic=\"enclave:{eid}\"")),
        "room page in an enclave must subscribe to the enclave topic",
    );
    assert!(
        body.contains(&format!("id=\"sidebar-nav-{eid}\"")),
        "sidebar nav must be enclave-keyed so the room-list OOB swap lands only on viewers of this enclave",
    );
}

// LC-172: the settings page subscribes to the enclave topic and renders the
// member list inside the swappable #lc-enclave-settings-members region, so
// joins/kicks/role-changes update it live (the broadcasts landed in LC-170).
#[tokio::test]
async fn settings_page_is_wired_for_live_member_updates() {
    let t = app().await;
    let (eid, _alpha, _beta) = seed(&t).await;
    let (status, _, body) = get(
        &t.app,
        &t.alice_session,
        &format!("/enclave/{eid}/settings"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("data-lc-live-topic=\"enclave:{eid}\"")),
        "settings page must subscribe to the enclave topic for live member updates",
    );
    assert!(
        body.contains("id=\"lc-enclave-settings-members\""),
        "settings member list must live in the swappable region the OOB fragment targets",
    );
    // Owner (can_delete) still sees the kick control inside that region.
    assert!(body.contains("/kick"), "owner sees member controls");
}

#[tokio::test]
async fn redirect_skips_private_room_owner_is_not_in() {
    let t = app().await;
    let eid = db::enclave::create_enclave(&t.chat, "Acme2", None, &t.alice)
        .await
        .unwrap();
    // A private room (sorts first by name) Alice is NOT a member of, plus a
    // public room. The default must skip the private one (opening it would
    // 403), landing on the public room.
    let _private = db::chat::create_room(&t.chat, "aaa", None, "private", Some("c"), Some(eid))
        .await
        .unwrap();
    let public = db::chat::create_room(&t.chat, "zzz", None, "public", None, Some(eid))
        .await
        .unwrap();
    let (status, loc, _) = get(&t.app, &t.alice_session, &format!("/enclave/{eid}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        loc.as_deref(),
        Some(format!("/room/{public}").as_str()),
        "default room is the openable (public) one, not the inaccessible private room"
    );
}
