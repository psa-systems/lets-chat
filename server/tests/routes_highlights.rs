use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p =
                std::env::temp_dir().join(format!("lc-highlights-tests-{}", std::process::id()));
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
    peer_id: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app_with_two_users(viewer: &str, peer: &str) -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, viewer, "hash").await.unwrap();
    let peer_id = db::auth::create_user(&auth, peer, "hash").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id = ?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let auth_for_test = auth.clone();
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
    let app = routes::build_router(state);
    TestApp {
        app,
        viewer_id,
        viewer_session,
        peer_id,
        auth: auth_for_test,
        chat: chat_for_test,
    }
}

async fn seed_public_room(t: &TestApp, name: &str) -> i64 {
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, name, None, "public", None, Some(general_id))
        .await
        .unwrap()
}

async fn seed_message(t: &TestApp, room_id: i64, user_id: &str, body: &str) -> i64 {
    db::chat::insert_message(&t.chat, room_id, user_id, body)
        .await
        .unwrap()
}

async fn react(t: &TestApp, message_id: i64, user_id: &str, emoji: &str) {
    sqlx::query("INSERT INTO message_reactions (message_id, user_id, emoji) VALUES (?, ?, ?)")
        .bind(message_id)
        .bind(user_id)
        .bind(emoji)
        .execute(&t.chat)
        .await
        .unwrap();
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
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).into_owned();
    (status, body)
}

// LC-903: a highlighted message authored by a blocked peer must not surface
// on the room's highlights recap, mirroring the room timeline.
#[tokio::test]
async fn blocked_author_message_absent_from_highlights() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "general-room").await;
    let msg = seed_message(&t, room, &t.peer_id, "most reacted from a blocked peer").await;
    react(&t, msg, &t.viewer_id, "tada").await;
    db::auth::block_user(&t.auth, &t.viewer_id, &t.peer_id)
        .await
        .unwrap();

    let (status, body) = get(
        &t.app,
        &t.viewer_session,
        &format!("/room/{room}/highlights"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("most reacted from a blocked peer"),
        "blocked author's message must not appear in highlights, got: {body}"
    );
}
