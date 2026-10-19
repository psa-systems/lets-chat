//! LC-1057: status "Clear after" resets the whole status and survives a
//! re-save, the save response keeps the account menu, and the DM "Seen" time
//! renders as a browser-localized `<time data-lc-ts>`.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-1057-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    app: Router,
    auth: SqlitePool,
    chat: SqlitePool,
    alice: String,
    alice_session: String,
    bob: String,
    bob_session: String,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let auth_for_test = auth.clone();
    let chat_for_test = chat.clone();
    let state = AppState {
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
    Fixture {
        app: routes::build_router(state),
        auth: auth_for_test,
        chat: chat_for_test,
        alice,
        alice_session,
        bob,
        bob_session,
    }
}

async fn send(
    app: &Router,
    method: Method,
    session: &str,
    uri: &str,
    form: Option<&str>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={session}"))
        .header("hx-request", "true");
    if form.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let res = app
        .clone()
        .oneshot(
            req.body(Body::from(form.unwrap_or("").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn expiry(f: &Fixture) -> Option<String> {
    db::auth::get_custom_status_expiry(&f.auth, &f.alice)
        .await
        .unwrap()
}

#[tokio::test]
async fn away_with_no_text_schedules_a_clear_and_keep_survives_a_resave() {
    let f = setup().await;
    let (st, body) = send(
        &f.app,
        Method::POST,
        &f.alice_session,
        "/status",
        Some("status=away&custom_status=&clear_after=30m"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let first = expiry(&f).await;
    assert!(first.is_some(), "Away alone is clearable now");

    // The save refreshes the whole account-menu block, not a bare button.
    assert!(
        body.contains(r#"id="sidebar-self" class="lc-rail-self" hx-swap-oob="outerHTML""#),
        "{body}"
    );
    assert!(
        body.contains(r#"aria-haspopup="menu""#),
        "the account menu trigger survives"
    );

    let (_, picker) = send(
        &f.app,
        Method::GET,
        &f.alice_session,
        "/status/picker",
        None,
    )
    .await;
    assert!(
        picker.contains(r#"<option value="keep" selected>"#),
        "{picker}"
    );
    assert!(
        !picker.contains("disabled"),
        "Clear after is usable without custom text"
    );

    send(
        &f.app,
        Method::POST,
        &f.alice_session,
        "/status",
        Some("status=away&custom_status=lunch&clear_after=keep"),
    )
    .await;
    assert_eq!(expiry(&f).await, first, "keep leaves the running timer");

    send(
        &f.app,
        Method::POST,
        &f.alice_session,
        "/status",
        Some("status=away&custom_status=lunch&clear_after="),
    )
    .await;
    assert_eq!(expiry(&f).await, None, "Don't clear removes it");
}

#[tokio::test]
async fn plain_online_has_nothing_to_clear() {
    let f = setup().await;
    send(
        &f.app,
        Method::POST,
        &f.alice_session,
        "/status",
        Some("status=active&custom_status=&clear_after=1h"),
    )
    .await;
    assert_eq!(expiry(&f).await, None);
}

#[tokio::test]
async fn dm_seen_time_renders_as_a_localized_time_element() {
    let f = setup().await;
    let dm = db::chat::create_dm_room(&f.chat, "@bob", &f.alice, &f.bob)
        .await
        .unwrap();
    db::chat::insert_message(&f.chat, dm.id, &f.alice, "are you online?")
        .await
        .unwrap();
    // Bob opening the DM marks it read.
    let (st, _) = send(
        &f.app,
        Method::GET,
        &f.bob_session,
        &format!("/dm/{}", f.alice),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (_, page) = send(
        &f.app,
        Method::GET,
        &f.alice_session,
        &format!("/dm/{}", f.bob),
        None,
    )
    .await;
    let at = page
        .find("Seen <time data-lc-ts data-lc-ts-mode=\"time\" datetime=\"")
        .expect(&page);
    let iso = &page[at..].split("datetime=\"").nth(1).unwrap()[..20];
    assert!(
        iso.ends_with('Z') && iso.as_bytes()[10] == b'T',
        "ISO UTC for the browser, got {iso}"
    );
}
