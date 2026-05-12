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
            let p = std::env::temp_dir().join(format!("lc-bookmark-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
            include_str!("../migrations/auth/0001_create_tables.sql"),
            include_str!("../migrations/auth/0002_read_receipts.sql"),
            include_str!("../migrations/auth/0003_profile_fields.sql"),
            include_str!("../migrations/auth/0004_user_status.sql"),
            include_str!("../migrations/auth/0005_profile_visibility.sql"),
            include_str!("../migrations/auth/0006_user_blocks.sql"),
            include_str!("../migrations/auth/0007_notification_settings.sql"),
            include_str!("../migrations/auth/0008_two_factor.sql"),
            include_str!("../migrations/auth/0009_push_subscriptions.sql"),
            include_str!("../migrations/auth/0010_password_reset.sql"),
            include_str!("../migrations/auth/0011_email_verification.sql"),
            include_str!("../migrations/auth/0012_session_metadata.sql"),
        ],
        "chat" => vec![
            include_str!("../migrations/chat/0001_create_tables.sql"),
            include_str!("../migrations/chat/0002_moderation.sql"),
            include_str!("../migrations/chat/0003_dms.sql"),
            include_str!("../migrations/chat/0004_message_editing.sql"),
            include_str!("../migrations/chat/0005_private_rooms.sql"),
            include_str!("../migrations/chat/0006_read_receipts.sql"),
            include_str!("../migrations/chat/0007_reactions.sql"),
            include_str!("../migrations/chat/0008_search.sql"),
            include_str!("../migrations/chat/0009_enclaves.sql"),
            include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
            include_str!("../migrations/chat/0011_threads.sql"),
            include_str!("../migrations/chat/0012_uploads.sql"),
            include_str!("../migrations/chat/0013_link_previews.sql"),
            include_str!("../migrations/chat/0014_mentions.sql"),
            include_str!("../migrations/chat/0015_room_notification_settings.sql"),
            include_str!("../migrations/chat/0016_pinned_messages.sql"),
            include_str!("../migrations/chat/0017_custom_emojis.sql"),
            include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
            include_str!("../migrations/chat/0019_bookmarks.sql"),
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

struct TestApp {
    app: Router,
    viewer_id: String,
    viewer_session: String,
    outsider_session: String,
    chat: SqlitePool,
}

async fn app_with_two_users() -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, "viewer", "hash")
        .await
        .unwrap();
    let outsider_id = db::auth::create_user(&auth, "outsider", "hash")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id = ?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    let outsider_session = db::auth::create_session(&auth, &outsider_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        secret_key: None,
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        viewer_id,
        viewer_session,
        outsider_session,
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

async fn seed_private_room(t: &TestApp, name: &str) -> i64 {
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::chat::create_room(
        &t.chat,
        name,
        None,
        "private",
        Some(&t.viewer_id),
        Some(general_id),
    )
    .await
    .unwrap()
}

async fn seed_message(t: &TestApp, room_id: i64, user_id: &str, body: &str) -> i64 {
    db::chat::insert_message(&t.chat, room_id, user_id, body)
        .await
        .unwrap()
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn post_bookmark_returns_rerendered_bubble_with_unsave_button() {
    let t = app_with_two_users().await;
    let room = seed_public_room(&t, "general-room").await;
    let msg = seed_message(&t, room, &t.viewer_id, "save me").await;

    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/bookmark"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains(&format!(r#"id="msg-{msg}""#)),
        "bubble missing: {body}"
    );
    // After save, the hover menu must show Unsave (DELETE), not Save (POST).
    assert!(
        body.contains(&format!(r#"hx-delete="/messages/{msg}/bookmark""#)),
        "Unsave button missing: {body}"
    );
    assert!(!body.contains(&format!(r#"hx-post="/messages/{msg}/bookmark""#)));

    // Database reflects the bookmark.
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks WHERE user_id = ? AND message_id = ?")
            .bind(&t.viewer_id)
            .bind(msg)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn delete_bookmark_flips_button_back_to_save() {
    let t = app_with_two_users().await;
    let room = seed_public_room(&t, "r").await;
    let msg = seed_message(&t, room, &t.viewer_id, "x").await;
    send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/bookmark"),
    )
    .await;

    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::DELETE,
        &format!("/messages/{msg}/bookmark"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(r#"hx-post="/messages/{msg}/bookmark""#)),
        "Save button missing after unsave: {body}"
    );
}

#[tokio::test]
async fn bookmark_is_idempotent() {
    let t = app_with_two_users().await;
    let room = seed_public_room(&t, "r").await;
    let msg = seed_message(&t, room, &t.viewer_id, "x").await;

    for _ in 0..3 {
        let (status, _) = send(
            &t.app,
            &t.viewer_session,
            Method::POST,
            &format!("/messages/{msg}/bookmark"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks WHERE user_id = ?")
        .bind(&t.viewer_id)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn bookmark_in_inaccessible_private_room_returns_403() {
    let t = app_with_two_users().await;
    let room = seed_private_room(&t, "secret").await;
    let msg = seed_message(&t, room, &t.viewer_id, "x").await;

    let (status, _body) = send(
        &t.app,
        &t.outsider_session,
        Method::POST,
        &format!("/messages/{msg}/bookmark"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bookmark_nonexistent_message_returns_404() {
    let t = app_with_two_users().await;
    let (status, _) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        "/messages/99999/bookmark",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bookmark_anonymous_redirects_or_unauthorized() {
    let t = app_with_two_users().await;
    let room = seed_public_room(&t, "r").await;
    let msg = seed_message(&t, room, &t.viewer_id, "x").await;

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/messages/{msg}/bookmark"))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::UNAUTHORIZED
            || status == StatusCode::SEE_OTHER
            || status == StatusCode::TEMPORARY_REDIRECT
            || status == StatusCode::FOUND,
        "unexpected status: {status}"
    );
}

#[tokio::test]
async fn saved_page_lists_bookmarked_messages_newest_first() {
    let t = app_with_two_users().await;
    let room = seed_public_room(&t, "r").await;
    let first = seed_message(&t, room, &t.viewer_id, "older one").await;
    let second = seed_message(&t, room, &t.viewer_id, "newer one").await;

    send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{first}/bookmark"),
    )
    .await;
    send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{second}/bookmark"),
    )
    .await;

    let (status, body) = send(&t.app, &t.viewer_session, Method::GET, "/saved").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("older one"), "older not listed: {body}");
    assert!(body.contains("newer one"), "newer not listed: {body}");
    // Newest first - "newer one" should appear before "older one".
    let p_new = body.find("newer one").unwrap();
    let p_old = body.find("older one").unwrap();
    assert!(p_new < p_old, "ordering wrong: newer={p_new} older={p_old}");
}

#[tokio::test]
async fn saved_page_empty_when_no_bookmarks() {
    let t = app_with_two_users().await;
    let (status, body) = send(&t.app, &t.viewer_session, Method::GET, "/saved").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("haven't saved any messages"),
        "empty-state missing: {body}"
    );
}

#[tokio::test]
async fn saved_page_hides_soft_deleted_messages() {
    let t = app_with_two_users().await;
    let room = seed_public_room(&t, "r").await;
    let msg = seed_message(&t, room, &t.viewer_id, "soon to vanish").await;
    send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/bookmark"),
    )
    .await;

    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(msg)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body) = send(&t.app, &t.viewer_session, Method::GET, "/saved").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("soon to vanish"),
        "soft-deleted message leaked: {body}"
    );
}
