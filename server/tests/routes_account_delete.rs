use axum::body::{to_bytes, Body};
use std::sync::Once;

fn init_tracing() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("error")
            .try_init()
            .ok();
    });
}
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir()
                .join(format!("lc-account-delete-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

fn hash(password: &str) -> String {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

struct TestApp {
    app: Router,
    user_id: String,
    session: String,
    peer_id: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app_with_user() -> TestApp {
    init_tracing();
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let user_id = db::auth::create_user(&auth, "alice", &hash(PASSWORD))
        .await
        .unwrap();
    let peer_id = db::auth::create_user(&auth, "bob", &hash(PASSWORD))
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
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
        bg: bg.clone(),
        secret_key: None,
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        user_id,
        session,
        peer_id,
        auth,
        chat,
    }
}

async fn post_delete(
    app: &Router,
    sess: &str,
    body: &str,
) -> (StatusCode, Vec<u8>, http::HeaderMap) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/delete-account")
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 4 << 20).await.unwrap().to_vec();
    (status, bytes, headers)
}

fn form(password: &str, phrase: &str) -> String {
    // Tests only use ASCII letters and spaces in these fields, so the only
    // transform needed for application/x-www-form-urlencoded is space->`+`.
    fn enc(s: &str) -> String {
        for b in s.bytes() {
            assert!(
                b.is_ascii_alphanumeric() || matches!(b, b' '),
                "form encoder does not handle byte {b:#x} in {s:?}"
            );
        }
        s.replace(' ', "+")
    }
    format!("password={}&confirm_phrase={}", enc(password), enc(phrase))
}

#[tokio::test]
async fn delete_wipes_user_and_chat_rows() {
    let t = app_with_user().await;

    // Seed data the deletion must clear: a message, a reaction, a
    // bookmark, a block, an extra session and a push subscription.
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let room_id = db::chat::create_room(
        &t.chat,
        "delete-room",
        None,
        "public",
        None,
        Some(general_id),
    )
    .await
    .unwrap();
    let msg = db::chat::insert_message(&t.chat, room_id, &t.user_id, "hello")
        .await
        .unwrap();
    sqlx::query("INSERT INTO message_reactions (message_id, user_id, emoji) VALUES (?, ?, ?)")
        .bind(msg)
        .bind(&t.user_id)
        .bind("👍")
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bookmarks (user_id, message_id) VALUES (?, ?)")
        .bind(&t.user_id)
        .bind(msg)
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (?, ?)")
        .bind(&t.user_id)
        .bind(&t.peer_id)
        .execute(&t.auth)
        .await
        .unwrap();
    let _extra_session = db::auth::create_session(&t.auth, &t.user_id).await.unwrap();
    sqlx::query(
        "INSERT INTO push_subscriptions (user_id, endpoint, p256dh_key, auth_key) \
         VALUES (?, 'https://example.invalid/p', 'k', 'a')",
    )
    .bind(&t.user_id)
    .execute(&t.auth)
    .await
    .unwrap();

    let (status, body, headers) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert!(
        status.is_redirection(),
        "expected redirect, got {status}: {}",
        String::from_utf8_lossy(&body)
    );
    let location = headers.get(header::LOCATION).unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
    let cookie = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with("session="))
        .expect("session cookie cleared");
    assert!(
        cookie.contains("Max-Age=0") || cookie.contains("expires=Thu, 01 Jan 1970"),
        "unexpected cookie: {cookie}"
    );

    // User row gone.
    let still_there: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&t.user_id)
        .fetch_optional(&t.auth)
        .await
        .unwrap();
    assert!(still_there.is_none(), "user row remained");

    // All sessions for the user gone (FK CASCADE).
    let n_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = ?")
        .bind(&t.user_id)
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert_eq!(n_sessions, 0);

    // Push subs gone (manual delete).
    let n_push: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_subscriptions WHERE user_id = ?")
            .bind(&t.user_id)
            .fetch_one(&t.auth)
            .await
            .unwrap();
    assert_eq!(n_push, 0);

    // Blocks gone (FK CASCADE).
    let n_blocks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_blocks WHERE blocker_id = ?")
        .bind(&t.user_id)
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert_eq!(n_blocks, 0);

    // Chat-side: messages, reactions, bookmarks, room_members, enclave_members.
    let n_msgs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE user_id = ?")
        .bind(&t.user_id)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n_msgs, 0);
    let n_reactions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_reactions WHERE user_id = ?")
            .bind(&t.user_id)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(n_reactions, 0);
    let n_bookmarks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks WHERE user_id = ?")
        .bind(&t.user_id)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n_bookmarks, 0);
    let n_enc: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM enclave_members WHERE user_id = ?")
        .bind(&t.user_id)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n_enc, 0);
}

#[tokio::test]
async fn delete_rejects_wrong_password() {
    let t = app_with_user().await;
    let (status, body, _) =
        post_delete(&t.app, &t.session, &form("nope", "delete my account")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("incorrect"), "body: {text}");
    let still: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&t.user_id)
        .fetch_optional(&t.auth)
        .await
        .unwrap();
    assert!(still.is_some(), "user must remain on wrong password");
}

#[tokio::test]
async fn delete_rejects_wrong_phrase() {
    let t = app_with_user().await;
    let (status, _, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "yes please delete")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let still: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&t.user_id)
        .fetch_optional(&t.auth)
        .await
        .unwrap();
    assert!(still.is_some());
}

#[tokio::test]
async fn delete_refuses_when_sole_owner_with_other_members() {
    let t = app_with_user().await;

    let enclave_id = db::enclave::create_enclave(&t.chat, "my-club", None, &t.user_id)
        .await
        .unwrap();
    // Add the peer as a member so the user is the sole owner of an
    // enclave that still has someone else in it.
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, 'member')")
        .bind(enclave_id)
        .bind(&t.peer_id)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let text = String::from_utf8_lossy(&body);
    assert!(text.to_lowercase().contains("my-club"), "body: {text}");

    // User still present, enclave still present.
    let still: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&t.user_id)
        .fetch_optional(&t.auth)
        .await
        .unwrap();
    assert!(still.is_some());
    let enc_still: Option<i64> = sqlx::query_scalar("SELECT id FROM enclaves WHERE id = ?")
        .bind(enclave_id)
        .fetch_optional(&t.chat)
        .await
        .unwrap();
    assert!(enc_still.is_some());
}

#[tokio::test]
async fn delete_refuses_when_caller_is_sole_admin_with_other_users() {
    let t = app_with_user().await;
    sqlx::query("UPDATE users SET role='admin' WHERE id = ?")
        .bind(&t.user_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let (status, body, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let text = String::from_utf8_lossy(&body);
    assert!(text.to_lowercase().contains("only admin"), "body: {text}");

    let still: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&t.user_id)
        .fetch_optional(&t.auth)
        .await
        .unwrap();
    assert!(still.is_some());
}

#[tokio::test]
async fn delete_allowed_when_another_admin_exists() {
    let t = app_with_user().await;
    sqlx::query("UPDATE users SET role='admin' WHERE id IN (?, ?)")
        .bind(&t.user_id)
        .bind(&t.peer_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let (status, body, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert!(
        status.is_redirection(),
        "expected redirect, got {status}: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn delete_allowed_when_caller_is_sole_user() {
    let t = app_with_user().await;
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&t.peer_id)
        .execute(&t.auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id = ?")
        .bind(&t.user_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let (status, body, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert!(
        status.is_redirection(),
        "expected redirect, got {status}: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn delete_drops_solo_owned_enclave() {
    let t = app_with_user().await;
    let enclave_id = db::enclave::create_enclave(&t.chat, "solo-club", None, &t.user_id)
        .await
        .unwrap();

    let (status, _, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert!(status.is_redirection(), "got {status}");

    let enc_still: Option<i64> = sqlx::query_scalar("SELECT id FROM enclaves WHERE id = ?")
        .bind(enclave_id)
        .fetch_optional(&t.chat)
        .await
        .unwrap();
    assert!(enc_still.is_none(), "solo-owned enclave must be removed");
}
