//! LC-549 integration: embeddings-backed related / semantic search.
//!
//! Uses the deterministic hashed mock embedder so shared-word messages rank as
//! similar. Covers: `GET /messages/{id}/related` surfaces the conceptually near
//! message and drops the unrelated one; the semantic search mode ranks by
//! meaning; and with no embeddings client the related endpoint is refused (search
//! degrades to FTS, tested elsewhere).

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::embeddings::{hash_embed, vec_to_bytes, MockEmbeddingClient};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-related-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    session: String,
    chat: SqlitePool,
}

async fn app(embeddings: bool) -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let session = db::auth::create_session(&auth, &alice).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let embedding_client: Option<Arc<dyn lets_chat::embeddings::EmbeddingClient>> = if embeddings {
        Some(Arc::new(MockEmbeddingClient::default()))
    } else {
        None
    };
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
        embedding_client,
    };
    db::enclave::create_enclave(&chat, "Acme", None, &alice)
        .await
        .unwrap();
    TestApp {
        app: routes::build_router(state),
        session,
        chat,
    }
}

async fn make_room(t: &TestApp) -> i64 {
    let eid: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name = 'Acme'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap()
}

/// Insert a message and store its deterministic mock embedding (mirrors what the
/// background populator does on send, without the spawn race).
async fn insert_embedded(t: &TestApp, room: i64, author: &str, body: &str) -> i64 {
    let id = db::chat::insert_message(&t.chat, room, author, body)
        .await
        .unwrap();
    let vec = hash_embed(body, 64);
    db::message_embeddings::upsert(&t.chat, id, room, vec.len() as i64, &vec_to_bytes(&vec))
        .await
        .unwrap();
    id
}

async fn get(t: &TestApp, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn related_surfaces_similar_and_drops_unrelated() {
    let t = app(true).await;
    let room = make_room(&t).await;
    let source = insert_embedded(&t, room, "bob", "the database migration failed on startup").await;
    insert_embedded(
        &t,
        room,
        "carol",
        "our database schema keeps breaking today",
    )
    .await;
    insert_embedded(&t, room, "dave", "who wants tacos for lunch this afternoon").await;

    let (status, body) = get(&t, &format!("/messages/{source}/related")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("database schema keeps breaking"),
        "related message should surface: {body}"
    );
    assert!(
        !body.contains("tacos for lunch"),
        "unrelated message should be filtered by the similarity floor: {body}"
    );
}

#[tokio::test]
async fn semantic_search_ranks_by_meaning() {
    let t = app(true).await;
    let room = make_room(&t).await;
    insert_embedded(&t, room, "bob", "the database migration failed on startup").await;
    insert_embedded(&t, room, "dave", "who wants tacos for lunch this afternoon").await;

    let (status, body) = get(
        &t,
        &format!("/search?room_id={room}&q=database%20problem&semantic=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("database migration failed"),
        "semantic hit should appear: {body}"
    );
    assert!(!body.contains("tacos for lunch"), "unrelated hit: {body}");
}

#[tokio::test]
async fn related_without_embeddings_is_rejected() {
    let t = app(false).await;
    let room = make_room(&t).await;
    let id = db::chat::insert_message(&t.chat, room, "bob", "hello there")
        .await
        .unwrap();
    let (status, _) = get(&t, &format!("/messages/{id}/related")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
