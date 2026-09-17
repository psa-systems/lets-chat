//! LC-927 (F10): the admin-forced reindex and the scheduled tick both funnel
//! through `help_docs::reindex_all`/`reindex_all_unchecked`, which must never
//! run two overlapping `reindex_all_inner` bodies: they would interleave
//! `delete_by_source`/`upsert` writes and race the source-prune "not visited
//! this run" computation. The guard is a `settings` row acquired with an
//! atomic `INSERT ... ON CONFLICT DO NOTHING` (`db::settings::
//! try_acquire_reindex_lock`), so a second run made while one is in flight is
//! rejected outright (a no-op default report) rather than run concurrently.
//!
//! The docs page handler below blocks on a `Notify` until the test releases
//! it, so the "first run is still in flight" window is deterministic instead
//! of relying on a sleep race.

use std::sync::Arc;

use axum::response::Html;
use axum::routing::get;
use axum::Router;
use lets_chat::db;
use lets_chat::embeddings::MockEmbeddingClient;
use lets_chat::llm::MockLlmClient;
use lets_chat::routes::help_docs;
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use tokio::net::TcpListener;
use tokio::sync::Notify;

mod common;

async fn state_with_embeddings() -> AppState {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
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
        llm_client: Some(Arc::new(MockLlmClient {
            canned: "unused".into(),
        })),
        embedding_client: Some(Arc::new(MockEmbeddingClient::default())),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat-help-docs/test")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn index_handler() -> Html<&'static str> {
    Html(r#"<html><body><a href="/docs/page1">Page 1</a></body></html>"#)
}

/// A single docs page whose handler signals `hit` the instant it is called,
/// then blocks until `release` is notified, holding the reindex run "in
/// flight" for as long as the test needs.
async fn spawn_blocking_docs_site() -> (String, Arc<Notify>, Arc<Notify>) {
    let hit = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hit2 = hit.clone();
    let release2 = release.clone();
    let page1_handler = move || {
        let hit = hit2.clone();
        let release = release2.clone();
        async move {
            hit.notify_one();
            release.notified().await;
            Html(
                r#"<html><head><title>Page One</title></head><body>
                <article class="docs-article"><h1>Page One</h1>
                <p>Widgets and gadgets: how to configure the widget subsystem.</p>
                </article></body></html>"#,
            )
        }
    };
    let app = Router::new()
        .route("/docs", get(index_handler))
        .route("/docs/page1", get(page1_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/docs"), hit, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reindex_in_flight_rejects_a_concurrent_one() {
    let state = state_with_embeddings().await;
    let (index_url, hit, release) = spawn_blocking_docs_site().await;
    db::settings::set_setting(
        &state.settings,
        help_docs::HELP_DOCS_SOURCES_KEY,
        &format!("widgets|{index_url}"),
    )
    .await
    .unwrap();

    let first_state = state.clone();
    let first = tokio::spawn(async move {
        help_docs::reindex_all_unchecked(&first_state, true, &client()).await
    });

    // Wait until the first run has genuinely acquired the lock and is
    // mid-fetch, so the second call below races against a real in-flight run
    // rather than a hypothetical one.
    hit.notified().await;

    let second = help_docs::reindex_all_unchecked(&state, true, &client())
        .await
        .unwrap();
    assert_eq!(
        second.pages, 0,
        "a reindex started while one is in flight must do no work"
    );
    assert_eq!(
        second.chunks, 0,
        "a rejected reindex must not write any chunks"
    );

    release.notify_one();
    let first = first.await.unwrap().unwrap();
    assert_eq!(first.pages, 1, "the in-flight run completes normally");

    let urls: Vec<String> = sqlx::query_scalar("SELECT DISTINCT source_url FROM doc_chunks")
        .fetch_all(&state.chat)
        .await
        .unwrap();
    assert_eq!(
        urls,
        vec![format!("{index_url}/page1")],
        "only the completed run's page was ever indexed, got: {urls:?}"
    );

    // The lock is released once its holder finishes, so a later run proceeds.
    assert!(
        db::settings::try_acquire_reindex_lock(&state.settings)
            .await
            .unwrap(),
        "the lock must be free again after the in-flight run finished"
    );
}

#[tokio::test]
async fn lock_acquire_release_is_a_simple_mutex() {
    let settings = common::pool("settings").await;
    assert!(db::settings::try_acquire_reindex_lock(&settings)
        .await
        .unwrap());
    assert!(
        !db::settings::try_acquire_reindex_lock(&settings)
            .await
            .unwrap(),
        "a second acquire while held must fail"
    );
    db::settings::release_reindex_lock(&settings).await.unwrap();
    assert!(
        db::settings::try_acquire_reindex_lock(&settings)
            .await
            .unwrap(),
        "released lock can be re-acquired"
    );
}
