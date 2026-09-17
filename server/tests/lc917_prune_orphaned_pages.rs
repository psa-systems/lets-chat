//! LC-917: a page that leaves its source's docs index must have its stored
//! chunks pruned on the next index run, not linger forever citing a dead URL.
//!
//! Spawns a local docs site (index page + two sub-pages) and points
//! `help_docs::reindex_all_unchecked` (test seam that skips the LC-152 SSRF
//! re-resolve so loopback works, mirrors `bridge_avatar_fetch`'s
//! `fetch_and_cache_unchecked`) at it. First run indexes both pages; the index
//! HTML is then rewritten to drop the second page's link and the run repeats,
//! asserting only the first page's chunks remain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use lets_chat::embeddings::MockEmbeddingClient;
use lets_chat::llm::MockLlmClient;
use lets_chat::routes::help_docs;
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, embeddings};
use tokio::net::TcpListener;

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

#[derive(Clone)]
struct Fixture {
    include_page2: Arc<AtomicBool>,
}

async fn index_handler(State(s): State<Fixture>) -> Html<String> {
    let mut html = String::from(r#"<html><body><a href="/docs/page1">Page 1</a>"#);
    if s.include_page2.load(Ordering::SeqCst) {
        html.push_str(r#"<a href="/docs/page2">Page 2</a>"#);
    }
    html.push_str("</body></html>");
    Html(html)
}

async fn page1_handler() -> Html<&'static str> {
    Html(
        r#"<html><head><title>Page One</title></head><body>
        <article class="docs-article"><h1>Page One</h1>
        <p>Widgets and gadgets: how to configure the widget subsystem.</p>
        </article></body></html>"#,
    )
}

async fn page2_handler() -> Html<&'static str> {
    Html(
        r#"<html><head><title>Page Two</title></head><body>
        <article class="docs-article"><h1>Page Two</h1>
        <p>Sprockets and gears: how to configure the gear subsystem.</p>
        </article></body></html>"#,
    )
}

/// Spawns the local docs site and returns its index URL
/// (`http://127.0.0.1:PORT/docs`) plus the flag controlling whether the index
/// page links Page 2.
async fn spawn_docs_site() -> (String, Arc<AtomicBool>) {
    let include_page2 = Arc::new(AtomicBool::new(true));
    let fixture = Fixture {
        include_page2: include_page2.clone(),
    };
    let app = Router::new()
        .route("/docs", get(index_handler))
        .route("/docs/page1", get(page1_handler))
        .route("/docs/page2", get(page2_handler))
        .with_state(fixture);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/docs"), include_page2)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat-help-docs/test")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn page_removed_from_index_is_pruned_on_next_run() {
    let state = state_with_embeddings().await;
    let (index_url, include_page2) = spawn_docs_site().await;
    db::settings::set_setting(
        &state.settings,
        help_docs::HELP_DOCS_SOURCES_KEY,
        &format!("widgets|{index_url}"),
    )
    .await
    .unwrap();

    let report = help_docs::reindex_all_unchecked(&state, false, &client())
        .await
        .unwrap();
    assert_eq!(report.pages, 2, "both pages indexed on the first run");
    let urls: Vec<String> = sqlx::query_scalar("SELECT DISTINCT source_url FROM doc_chunks")
        .fetch_all(&state.chat)
        .await
        .unwrap();
    assert_eq!(urls.len(), 2, "both pages' chunks stored, got: {urls:?}");

    // Page 2 drops out of the index (removed or renamed upstream).
    include_page2.store(false, Ordering::SeqCst);

    let report = help_docs::reindex_all_unchecked(&state, false, &client())
        .await
        .unwrap();
    assert_eq!(
        report.removed_pages, 1,
        "the dropped page is counted as pruned"
    );

    let remaining: Vec<String> = sqlx::query_scalar("SELECT DISTINCT source_url FROM doc_chunks")
        .fetch_all(&state.chat)
        .await
        .unwrap();
    assert_eq!(
        remaining,
        vec![format!("{index_url}/page1")],
        "only page1's chunks remain, got: {remaining:?}"
    );
}

#[tokio::test]
async fn failed_index_fetch_prunes_nothing() {
    let state = state_with_embeddings().await;
    let (index_url, _include_page2) = spawn_docs_site().await;
    db::settings::set_setting(
        &state.settings,
        help_docs::HELP_DOCS_SOURCES_KEY,
        &format!("widgets|{index_url}"),
    )
    .await
    .unwrap();
    help_docs::reindex_all_unchecked(&state, false, &client())
        .await
        .unwrap();

    // Point the source at an unreachable index (index fetch fails outright);
    // the previously stored pages must survive untouched.
    db::settings::set_setting(
        &state.settings,
        help_docs::HELP_DOCS_SOURCES_KEY,
        "widgets|http://127.0.0.1:1/docs",
    )
    .await
    .unwrap();
    let report = help_docs::reindex_all_unchecked(&state, false, &client())
        .await
        .unwrap();
    assert_eq!(
        report.removed_pages, 0,
        "a failed index fetch prunes nothing"
    );
    let n = db::doc_chunks::count(&state.chat).await.unwrap();
    assert!(n > 0, "existing chunks survive a failed index fetch");
}

#[tokio::test]
async fn page_with_no_sections_is_pruned_and_counted_as_an_error() {
    let state = state_with_embeddings().await;

    // A minimal site whose single sub-page has no extractable sections (an
    // empty body), which must be pruned + counted as an error, not indexed.
    async fn empty_index() -> Html<&'static str> {
        Html(r#"<html><body><a href="/docs/blank">Blank</a></body></html>"#)
    }
    async fn empty_page() -> Html<&'static str> {
        Html(r#"<html><head><title>Blank</title></head><body></body></html>"#)
    }
    let app = Router::new()
        .route("/docs", get(empty_index))
        .route("/docs/blank", get(empty_page));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let index_url = format!("http://{addr}/docs");

    db::settings::set_setting(
        &state.settings,
        help_docs::HELP_DOCS_SOURCES_KEY,
        &format!("widgets|{index_url}"),
    )
    .await
    .unwrap();

    // Seed a stale chunk directly under that URL, as if a prior run (before
    // the markup regressed) had indexed real content there.
    let mock = embeddings::MockEmbeddingClient::default();
    let vec = embeddings::EmbeddingClient::embed(&mock, "Blank\nold content")
        .await
        .unwrap();
    db::doc_chunks::upsert(
        &state.chat,
        "widgets",
        &format!("{index_url}/blank"),
        "Blank",
        "Blank",
        0,
        "old content",
        "oldhash",
        embeddings::EmbeddingClient::model_name(&mock),
        vec.len() as i64,
        &embeddings::vec_to_bytes(&vec),
    )
    .await
    .unwrap();

    let report = help_docs::reindex_all_unchecked(&state, false, &client())
        .await
        .unwrap();
    assert_eq!(
        report.errors, 1,
        "the empty-sections page counts as an error"
    );
    assert_eq!(report.pages, 0, "not counted as indexed");
    let n = db::doc_chunks::count(&state.chat).await.unwrap();
    assert_eq!(n, 0, "its stale chunks are removed");
}
