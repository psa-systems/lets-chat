//! LC-782: the switcher (F6) and the Inbox/Activity list pages (F10) used to
//! resolve one branding/membership row per enclave, and one author/room/DM-peer
//! row per list item, in a loop - so their query cost scaled with the enclave
//! count / page size instead of staying fixed. These tests pin the fixed-cost
//! shape with a `tracing::Subscriber` that captures every `sqlx::query` event
//! (LC-911's `CapturingSubscriber`, reused from `tests/common/mod.rs`), so a
//! regression back to a per-row loop shows up as a growing match count rather
//! than only a slower page.
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-782-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    viewer_id: String,
    viewer_session: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, "viewer", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let auth_for_test = auth.clone();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
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
        bg: bg.clone(),
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
    let app = routes::build_router(state);
    TestApp {
        app,
        viewer_id,
        viewer_session,
        auth: auth_for_test,
        chat: chat_for_test,
    }
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body =
        String::from_utf8(to_bytes(resp.into_body(), 10 << 20).await.unwrap().to_vec()).unwrap();
    (status, body)
}

// `sqlx-sqlite` runs every query on a dedicated per-connection worker thread
// (see `sqlx-sqlite`'s `connection/worker.rs`), not on the async task's own
// thread, so `tracing::subscriber::set_default`'s thread-local override (what
// `common::CapturingSubscriber` is designed for) never reaches it: the worker
// thread falls back to the process-wide GLOBAL default instead. Installing the
// capture there means it stays live for the rest of this test binary and sees
// every thread, so access to the shared event log is serialized through
// `SERIAL` and drained before each render to keep the tests in this file from
// reading each other's queries.
static EVENTS: std::sync::LazyLock<std::sync::Arc<std::sync::Mutex<Vec<String>>>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
static SERIAL: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

struct GlobalCapturingSubscriber;

impl tracing::Subscriber for GlobalCapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct FieldsToString(String);
        impl tracing::field::Visit for FieldsToString {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(&format!("{}={:?}", field.name(), value));
            }
        }
        let mut visitor = FieldsToString(String::new());
        event.record(&mut visitor);
        EVENTS.lock().unwrap().push(visitor.0);
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

fn install_global_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::subscriber::set_global_default(GlobalCapturingSubscriber).ok();
    });
}

/// Render `uri` and return every `sqlx::query` event logged anywhere in the
/// process during that render.
async fn queries_for(app: &Router, sess: &str, uri: &str) -> (StatusCode, String, Vec<String>) {
    install_global_subscriber();
    let _guard = SERIAL.lock().await;
    EVENTS.lock().unwrap().clear();
    let (status, body) = get(app, sess, uri).await;
    let logged = EVENTS.lock().unwrap().clone();
    (status, body, logged)
}

fn matching(events: &[String], needle: &str) -> usize {
    events.iter().filter(|e| e.contains(needle)).count()
}

// ── F6: the switcher ────────────────────────────────────────────────────────

/// A user in 8 enclaves must cost the switcher a fixed 2 queries (the global
/// branding resolve plus one batched `logo_ids_for_enclaves` call), not one
/// `get_membership` + one `branding::resolve` per enclave.
#[tokio::test(flavor = "current_thread")]
async fn switcher_branding_and_membership_cost_is_fixed_regardless_of_enclave_count() {
    let t = app().await;
    // The viewer already belongs to General (from `backfill_general_membership`).
    // Add 7 more enclaves so the viewer is in 8 total.
    for i in 0..7 {
        let enclave_id =
            db::enclave::create_enclave(&t.chat, &format!("enclave-{i}"), None, "creator")
                .await
                .unwrap();
        db::enclave::add_member(
            &t.chat,
            enclave_id,
            &t.viewer_id,
            lets_chat::models::enclave::EnclaveRole::Member,
        )
        .await
        .unwrap();
    }

    let (status, _body, events) = queries_for(&t.app, &t.viewer_session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);

    // `get_membership`'s query shape must not appear anywhere in the render:
    // the switcher's per-tile role now rides on `list_enclaves_for_user_with_role`'s
    // JOIN instead of a separate query per enclave.
    assert_eq!(
        matching(&events, "FROM enclave_members WHERE enclave_id"),
        0,
        "get_membership's query shape must not run during page render, got: {events:?}"
    );
    // The batched branding lookup runs exactly once regardless of enclave count.
    assert_eq!(
        matching(
            &events,
            "FROM branding WHERE scope_kind = 'enclave' AND scope_id IN"
        ),
        1,
        "logo_ids_for_enclaves must run exactly once for all 8 enclaves, got: {events:?}"
    );
}

// ── F10: the list pages ─────────────────────────────────────────────────────

/// A full 30-row Inbox page must resolve every row's author and DM peer
/// through the batched helpers (one `users_by_ids` call, one
/// `dm_peers_for_rooms` call), not one `find_user_by_id` / `get_dm_peer` pair
/// per row.
#[tokio::test(flavor = "current_thread")]
async fn inbox_page_resolves_rows_in_a_fixed_number_of_queries() {
    let t = app().await;
    // 25 distinct authors post unread top-level messages in the seeded public
    // room 1 (General), plus 5 distinct DM peers each open a DM and post one
    // unread message there - 30 rows, all with distinct authors/peers so a
    // per-row loop would cost roughly one query per row.
    for i in 0..25 {
        let author = db::auth::create_user(&t.auth, &format!("author-{i}"), "h")
            .await
            .unwrap();
        db::enclave::backfill_general_membership(&t.auth, &t.chat)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (1, ?, ?)")
            .bind(&author)
            .bind(format!("msg {i}"))
            .execute(&t.chat)
            .await
            .unwrap();
    }
    for i in 0..5 {
        let peer = db::auth::create_user(&t.auth, &format!("dm-peer-{i}"), "h")
            .await
            .unwrap();
        let (room, _) = db::chat::find_or_create_dm_room(&t.chat, "dm", &t.viewer_id, &peer)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
            .bind(room.id)
            .bind(&peer)
            .bind(format!("dm {i}"))
            .execute(&t.chat)
            .await
            .unwrap();
    }

    let (status, body, events) = queries_for(&t.app, &t.viewer_session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("msg 0"), "expected the seeded rows to render");

    assert_eq!(
        matching(&events, "FROM users WHERE id IN"),
        1,
        "users_by_ids must resolve every row's author/peer in one call, got: {events:?}"
    );
    assert_eq!(
        matching(&events, "m1.room_id IN"),
        1,
        "dm_peers_for_rooms must resolve every DM row's peer in one call, got: {events:?}"
    );
}

/// A full 60-row Activity page must resolve every row's actor, room and DM
/// peer through the batched helpers, not one query per row.
#[tokio::test(flavor = "current_thread")]
async fn activity_page_resolves_rows_in_a_fixed_number_of_queries() {
    let t = app().await;
    // A root message from the viewer that 50 distinct peers reply to (mention
    // + reply activity, all in the same accessible room), plus 10 distinct DM
    // peers who reply to a DM root - 60 rows with distinct actors so a
    // per-row loop would cost roughly one (or more) query per row.
    let root: i64 = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body) VALUES (1, ?, 'root') RETURNING id",
    )
    .bind(&t.viewer_id)
    .fetch_one(&t.chat)
    .await
    .unwrap()
    .get(0);
    for i in 0..50 {
        let peer = db::auth::create_user(&t.auth, &format!("replier-{i}"), "h")
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages (room_id, user_id, body, parent_id) VALUES (1, ?, ?, ?)")
            .bind(&peer)
            .bind(format!("reply {i}"))
            .bind(root)
            .execute(&t.chat)
            .await
            .unwrap();
    }
    for i in 0..10 {
        let peer = db::auth::create_user(&t.auth, &format!("dm-replier-{i}"), "h")
            .await
            .unwrap();
        let (room, _) = db::chat::find_or_create_dm_room(&t.chat, "dm", &t.viewer_id, &peer)
            .await
            .unwrap();
        let dm_root: i64 = sqlx::query(
            "INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, 'dm root') RETURNING id",
        )
        .bind(room.id)
        .bind(&t.viewer_id)
        .fetch_one(&t.chat)
        .await
        .unwrap()
        .get(0);
        sqlx::query("INSERT INTO messages (room_id, user_id, body, parent_id) VALUES (?, ?, ?, ?)")
            .bind(room.id)
            .bind(&peer)
            .bind(format!("dm reply {i}"))
            .bind(dm_root)
            .execute(&t.chat)
            .await
            .unwrap();
    }

    let (status, body, events) = queries_for(&t.app, &t.viewer_session, "/activity").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("@replier-0") || body.contains("replier-0"),
        "expected the seeded rows to render, got: {body}"
    );

    assert_eq!(
        matching(&events, "FROM users WHERE id IN"),
        1,
        "users_by_ids must resolve every row's actor/peer in one call, got: {events:?}"
    );
    assert_eq!(
        matching(&events, "FROM rooms WHERE id IN"),
        1,
        "rooms_by_ids must resolve every row's room in one call, got: {events:?}"
    );
    assert_eq!(
        matching(&events, "m1.room_id IN"),
        1,
        "dm_peers_for_rooms must resolve every DM row's peer in one call, got: {events:?}"
    );
}
