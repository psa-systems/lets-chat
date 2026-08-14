//! LC-712: AI help desk, Phase 1. Covers the docs-RAG answer path
//! (`help_docs::build_support_answer`): a seeded documentation chunk that shares
//! vocabulary with the question is retrieved, fed to the (mock) LLM, and the
//! answer carries a real `Sources` citation; a question with nothing relevant
//! indexed gets the honest "couldn't find it" low-confidence reply instead of a
//! guess. Uses the deterministic MockEmbeddingClient (shared words -> positive
//! cosine) so ranking is testable without a live model.

use std::sync::Arc;

use lets_chat::embeddings::{EmbeddingClient, MockEmbeddingClient};
use lets_chat::llm::MockLlmClient;
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, embeddings};

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
            canned: "Set the DATABASE_URL environment variable.".into(),
        })),
        embedding_client: Some(Arc::new(MockEmbeddingClient::default())),
    }
}

/// Seed one doc chunk, embedding it exactly as the indexer does
/// (`heading\nbody`) so the mock cosine ranking matches a live index.
async fn seed_chunk(
    state: &AppState,
    product: &str,
    url: &str,
    title: &str,
    heading: &str,
    body: &str,
) {
    let client = MockEmbeddingClient::default();
    let vec = client.embed(&format!("{heading}\n{body}")).await.unwrap();
    let bytes = embeddings::vec_to_bytes(&vec);
    db::doc_chunks::upsert(
        &state.chat,
        product,
        url,
        title,
        heading,
        0,
        body,
        "hash0",
        vec.len() as i64,
        &bytes,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn support_answer_cites_the_retrieved_doc() {
    let state = state_with_embeddings().await;
    seed_chunk(
        &state,
        "mokosh-server",
        "https://a8n.systems/apps/mokosh-server/docs/configuration",
        "Configuration",
        "Database",
        "Set DATABASE_URL to configure the postgres database connection string.",
    )
    .await;
    // An unrelated chunk that should not be cited (no shared vocabulary).
    seed_chunk(
        &state,
        "mokosh-www",
        "https://a8n.systems/apps/mokosh-www/docs/theming",
        "Theming",
        "Colors",
        "Pick an accent color palette for the marketing site header.",
    )
    .await;

    let llm = MockLlmClient {
        canned: "Set the DATABASE_URL environment variable to your postgres connection string."
            .into(),
    };
    let body = lets_chat::routes::help_docs::build_support_answer(
        &state,
        "how do I configure the postgres database connection?",
        "> alice: how do I configure the postgres database connection?",
        &llm,
    )
    .await;

    // The header, the model's answer, and a real citation to the retrieved doc.
    assert!(body.contains("> alice:"), "keeps the attribution header");
    assert!(
        body.contains("DATABASE_URL environment variable"),
        "includes the model answer, got: {body}"
    );
    assert!(body.contains("**Sources:**"), "renders a Sources block");
    assert!(
        body.contains("https://a8n.systems/apps/mokosh-server/docs/configuration"),
        "cites the retrieved doc URL, got: {body}"
    );
    assert!(
        body.contains("mokosh-server: Configuration"),
        "cites the product + title, got: {body}"
    );
    // The shared-vocabulary doc must outrank the unrelated one: its citation
    // comes first in the (rank-ordered) Sources block. (With the coarse 64-dim
    // mock both may clear the floor, so we assert ordering rather than exclusion;
    // the real model separates them further.)
    let cfg = body.find("configuration").expect("config cited");
    if let Some(theming) = body.find("theming") {
        assert!(cfg < theming, "relevant doc must rank first, got: {body}");
    }
}

#[tokio::test]
async fn support_answer_is_honest_when_nothing_relevant_is_indexed() {
    let state = state_with_embeddings().await;
    seed_chunk(
        &state,
        "mokosh-www",
        "https://a8n.systems/apps/mokosh-www/docs/theming",
        "Theming",
        "Colors",
        "Pick an accent color palette for the marketing site header.",
    )
    .await;

    let llm = MockLlmClient {
        canned: "This should never be shown.".into(),
    };
    let body = lets_chat::routes::help_docs::build_support_answer(
        &state,
        "how do I rotate the kubernetes signing certificates?",
        "> bob: how do I rotate the kubernetes signing certificates?",
        &llm,
    )
    .await;

    // Low-confidence: nothing above the relevance floor, so it declines honestly
    // and does NOT surface the LLM's canned answer.
    assert!(
        body.contains("couldn't find anything about that in the product documentation"),
        "declines honestly, got: {body}"
    );
    assert!(
        !body.contains("This should never be shown"),
        "must not call the model when retrieval is empty, got: {body}"
    );
    assert!(
        !body.contains("**Sources:**"),
        "no citations when nothing matched"
    );
}
