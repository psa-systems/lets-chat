//! LC-1024 (F14): the docs reindex now embeds a page's chunks through a
//! bounded-concurrency stream instead of one at a time. This must preserve the
//! existing all-or-nothing guarantee: a page whose embedding fails partway
//! through still leaves that page's previously stored chunks untouched, and
//! the failure is counted as an error rather than a partial index.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use lets_chat::db;
use lets_chat::embeddings::{EmbeddingClient, EmbeddingError, MockEmbeddingClient};
use lets_chat::llm::MockLlmClient;
use lets_chat::routes::help_docs;
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use sqlx::SqlitePool;
use tokio::net::TcpListener;

mod common;

/// Builds an `AppState` over the given (already-migrated) pools, so a test can
/// share one underlying database across two states that differ only in which
/// embedding client is wired in.
async fn state_with_pools(
    auth: SqlitePool,
    chat: SqlitePool,
    settings: SqlitePool,
    client: Arc<dyn EmbeddingClient>,
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
        llm_client: Some(Arc::new(MockLlmClient {
            canned: "unused".into(),
        })),
        embedding_client: Some(client),
    }
}

/// Embeds like [`MockEmbeddingClient`] but fails on the `fail_at`th call
/// (1-indexed) and every call after, simulating an embeddings endpoint that
/// goes down partway through a page with several chunks.
struct FlakyEmbeddingClient {
    inner: MockEmbeddingClient,
    calls: AtomicUsize,
    fail_at: usize,
}

#[async_trait]
impl EmbeddingClient for FlakyEmbeddingClient {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= self.fail_at {
            return Err(EmbeddingError::Transport(
                "embeddings endpoint unreachable".into(),
            ));
        }
        self.inner.embed(text).await
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

async fn index_page() -> Html<&'static str> {
    Html(
        r#"<html><head><title>Widgets</title></head><body>
        <article class="docs-article"><a href="/docs/widgets">Widgets</a></article>
        </body></html>"#,
    )
}

/// One page with two h2 sections, so `index_page` embeds two chunks for it.
async fn widgets_page() -> Html<&'static str> {
    Html(
        r#"<html><head><title>Widgets</title></head><body>
        <article class="docs-article">
        <h1>Widgets</h1>
        <h2>Configure</h2>
        <p>How to configure the widget subsystem end to end.</p>
        <h2>Troubleshoot</h2>
        <p>How to troubleshoot the widget subsystem when it misbehaves.</p>
        </article></body></html>"#,
    )
}

async fn spawn_docs_site() -> String {
    let app = Router::new()
        .route("/docs", get(index_page))
        .route("/docs/widgets", get(widgets_page));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/docs")
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat-help-docs/test")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_page_whose_embed_fails_leaves_its_stored_chunks_untouched() {
    let index_url = spawn_docs_site().await;
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    // First run: a healthy embeddings client indexes the page fully.
    let state = state_with_pools(
        auth.clone(),
        chat.clone(),
        settings.clone(),
        Arc::new(MockEmbeddingClient::default()),
    )
    .await;
    db::settings::set_setting(
        &state.settings,
        help_docs::HELP_DOCS_SOURCES_KEY,
        &format!("widgets|{index_url}"),
    )
    .await
    .unwrap();
    let report = help_docs::reindex_all_unchecked(&state, false, &http_client())
        .await
        .unwrap();
    assert_eq!(report.pages, 1, "the page indexes cleanly the first time");
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT heading, body FROM doc_chunks ORDER BY chunk_index")
            .fetch_all(&state.chat)
            .await
            .unwrap();
    assert_eq!(before.len(), 2, "both sections were stored as chunks");

    // Second run, forced (skips the content-hash skip): the embeddings client
    // fails on the second chunk of the page, after the first has already
    // resolved through the bounded-concurrency stream.
    let flaky = Arc::new(FlakyEmbeddingClient {
        inner: MockEmbeddingClient::default(),
        calls: AtomicUsize::new(0),
        fail_at: 2,
    });
    let state2 = state_with_pools(auth, chat, settings, flaky).await;
    let report2 = help_docs::reindex_all_unchecked(&state2, true, &http_client())
        .await
        .unwrap();
    assert_eq!(report2.errors, 1, "the flaky embed is counted as an error");
    assert_eq!(report2.pages, 0, "not counted as (re-)indexed");

    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT heading, body FROM doc_chunks ORDER BY chunk_index")
            .fetch_all(&state.chat)
            .await
            .unwrap();
    assert_eq!(
        after, before,
        "the page's chunks from the last successful index survive a mid-page embed failure untouched"
    );
}
