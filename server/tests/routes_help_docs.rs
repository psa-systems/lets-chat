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
use lets_chat::models::User;
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

#[tokio::test]
async fn human_escalation_dms_all_admins_and_reports_availability() {
    let state = state_with_embeddings().await;
    // A plain member (the requester) and two admins in the auth db.
    let req_id = db::auth::create_user(&state.auth, "member", "h")
        .await
        .unwrap();
    let requester: User = db::auth::find_user_by_id(&state.auth, &req_id)
        .await
        .unwrap()
        .unwrap()
        .into();
    let a1 = db::auth::create_user(&state.auth, "admin1", "h")
        .await
        .unwrap();
    let a2 = db::auth::create_user(&state.auth, "admin2", "h")
        .await
        .unwrap();
    for a in [&a1, &a2] {
        sqlx::query("UPDATE users SET role='admin' WHERE id=?")
            .bind(a)
            .execute(&state.auth)
            .await
            .unwrap();
    }
    // General (id=1) is seeded by the migration; backfill membership so it is a
    // usable room to escalate from.
    db::enclave::backfill_general_membership(&state.auth, &state.chat)
        .await
        .unwrap();
    let room = db::chat::get_room(&state.chat, 1)
        .await
        .unwrap()
        .expect("general room seeded");

    let outcome = lets_chat::routes::help_docs::escalate_to_admins(
        &state,
        &requester,
        &room,
        "my account is locked",
    )
    .await
    .unwrap();

    assert_eq!(outcome.notified, 2, "both admins notified");
    assert!(
        outcome.available,
        "a freshly-created admin counts as active"
    );

    // Each admin has a bot DM carrying the escalation and a link back to the room.
    let bot = db::auth::find_user_by_username(&state.auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created");
    for a in [&a1, &a2] {
        let dm = db::chat::find_dm_room(&state.chat, &bot.id, a)
            .await
            .unwrap()
            .expect("bot DM exists for the admin");
        let body: String = sqlx::query_scalar(
            "SELECT body FROM messages WHERE room_id=? ORDER BY id DESC LIMIT 1",
        )
        .bind(dm.id)
        .fetch_one(&state.chat)
        .await
        .unwrap();
        assert!(
            body.contains("Help requested"),
            "DM carries the escalation, got: {body}"
        );
        assert!(
            body.contains("my account is locked"),
            "DM carries the note, got: {body}"
        );
        assert!(
            body.contains("/room/1"),
            "DM links back to the origin room, got: {body}"
        );
    }
}

#[tokio::test]
async fn human_escalation_with_no_admins_notifies_nobody() {
    let state = state_with_embeddings().await;
    let req_id = db::auth::create_user(&state.auth, "member", "h")
        .await
        .unwrap();
    let requester: User = db::auth::find_user_by_id(&state.auth, &req_id)
        .await
        .unwrap()
        .unwrap()
        .into();
    db::enclave::backfill_general_membership(&state.auth, &state.chat)
        .await
        .unwrap();
    let room = db::chat::get_room(&state.chat, 1).await.unwrap().unwrap();

    let outcome = lets_chat::routes::help_docs::escalate_to_admins(&state, &requester, &room, "")
        .await
        .unwrap();
    assert_eq!(outcome.notified, 0, "no admins to notify");
    assert!(!outcome.available);
}
