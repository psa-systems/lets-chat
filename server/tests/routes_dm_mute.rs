use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::notifications::MuteMode;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-dm-mute-tests-{}", std::process::id()));
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
            include_str!("../migrations/auth/0013_digest_columns.sql"),
            include_str!("../migrations/auth/0014_login_alerts.sql"),
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
    session: String,
    viewer_id: String,
    peer_id: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

/// Build a router with `viewer` and `peer` registered. The viewer has an
/// active session cookie. No DM room is created yet - tests that need one
/// call `seed_dm_room` directly.
async fn app_with_two_users(viewer: &str, peer: &str) -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, viewer, "hash").await.unwrap();
    let peer_id = db::auth::create_user(&auth, peer, "hash").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id IN (?, ?)")
        .bind(&viewer_id)
        .bind(&peer_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let auth_for_test = auth.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        session,
        viewer_id,
        peer_id,
        auth: auth_for_test,
        chat: chat_for_test,
    }
}

async fn seed_dm_room(t: &TestApp) -> i64 {
    let r = db::chat::create_dm_room(&t.chat, "@peer", &t.viewer_id, &t.peer_id)
        .await
        .unwrap();
    r.id
}

async fn count_mute_rows(chat: &SqlitePool, user_id: &str, room_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_one(chat)
    .await
    .unwrap()
}

async fn post_mute_full(
    app: &Router,
    sess: &str,
    peer_id: &str,
    body: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/dm/{peer_id}/mute"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_dm_page(app: &Router, sess: &str, peer_id: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/dm/{peer_id}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_root(app: &Router, sess: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn post_persists_dm_mute_and_returns_swapped_header() {
    let t = app_with_two_users("viewer", "alice").await;
    let dm_id = seed_dm_room(&t).await;

    let (status, body) = post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"id="lc-dm-header""#),
        "header swap target missing: {body}"
    );
    assert!(
        body.contains(r#"name="muted""#) && body.contains("checked"),
        "expected checked Mute checkbox: {body}"
    );

    let stored: String = sqlx::query_scalar(
        "SELECT mute_mode FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind(&t.viewer_id)
    .bind(dm_id)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(stored, "all");
}

#[tokio::test]
async fn post_with_muted_absent_deletes_row() {
    let t = app_with_two_users("viewer", "alice").await;
    let dm_id = seed_dm_room(&t).await;

    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(count_mute_rows(&t.chat, &t.viewer_id, dm_id).await, 1);

    // Empty body: no `muted` field present == unchecked.
    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "").await.0,
        StatusCode::OK
    );
    assert_eq!(count_mute_rows(&t.chat, &t.viewer_id, dm_id).await, 0);
}

#[tokio::test]
async fn post_unauthenticated_redirects_or_unauthorized() {
    let t = app_with_two_users("viewer", "alice").await;
    seed_dm_room(&t).await;
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/dm/{}/mute", t.peer_id))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("muted=on"))
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::SEE_OTHER
            || resp.status() == StatusCode::TEMPORARY_REDIRECT
            || resp.status() == StatusCode::FOUND
            || resp.status() == StatusCode::UNAUTHORIZED,
        "status: {}",
        resp.status()
    );
}

#[tokio::test]
async fn post_with_unknown_peer_returns_404() {
    let t = app_with_two_users("viewer", "alice").await;
    let (status, _) = post_mute_full(&t.app, &t.session, "no-such-user", "muted=on").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_with_no_dm_room_returns_404() {
    // Peer exists but no DM room has been created yet.
    let t = app_with_two_users("viewer", "alice").await;
    let (status, _) = post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_self_dm_returns_400() {
    let t = app_with_two_users("viewer", "alice").await;
    let (status, _) = post_mute_full(&t.app, &t.session, &t.viewer_id, "muted=on").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dm_mute_per_direction_independent() {
    // Viewer muting their DM with peer must not write a mute row for peer.
    let t = app_with_two_users("viewer", "alice").await;
    let dm_id = seed_dm_room(&t).await;

    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );

    // Viewer has a row.
    assert_eq!(count_mute_rows(&t.chat, &t.viewer_id, dm_id).await, 1);
    // Peer does not.
    assert_eq!(count_mute_rows(&t.chat, &t.peer_id, dm_id).await, 0);
    // Read-back confirms.
    assert_eq!(
        db::notifications::room_mute_mode(&t.chat, &t.viewer_id, dm_id)
            .await
            .unwrap(),
        MuteMode::All
    );
    assert_eq!(
        db::notifications::room_mute_mode(&t.chat, &t.peer_id, dm_id)
            .await
            .unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn dm_mute_does_not_zero_unread_watermark() {
    let t = app_with_two_users("viewer", "alice").await;
    let dm_id = seed_dm_room(&t).await;

    // Mute first, then have peer post 3 messages.
    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );
    for i in 0..3 {
        sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
            .bind(dm_id)
            .bind(&t.peer_id)
            .bind(format!("msg {i}"))
            .execute(&t.chat)
            .await
            .unwrap();
    }

    // Viewer's read state has not been written; the watermark is 0.
    let state = db::chat::get_dm_read_state(&t.chat, &t.viewer_id, dm_id)
        .await
        .unwrap();
    assert!(state.is_none());

    // The 3 unread messages still exist; mute did not delete them.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(dm_id)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n, 3);

    // Opening the DM advances the watermark to the latest message.
    let (status, _body) = get_dm_page(&t.app, &t.session, &t.peer_id).await;
    assert_eq!(status, StatusCode::OK);
    let advanced = db::chat::get_dm_read_state(&t.chat, &t.viewer_id, dm_id)
        .await
        .unwrap()
        .expect("opening DM seeds read state");
    assert!(advanced.last_read_message_id >= 1);
}

#[tokio::test]
async fn sidebar_renders_muted_dm_with_greyed_class() {
    let t = app_with_two_users("viewer", "alice").await;
    seed_dm_room(&t).await;
    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );

    let (status, body) = get_root(&t.app, &t.session).await;
    assert_eq!(status, StatusCode::OK);
    let anchor_marker = format!(r#"href="/dm/{}""#, t.peer_id);
    let idx = body
        .find(&anchor_marker)
        .unwrap_or_else(|| panic!("DM peer anchor missing in body: {body}"));
    let rest = &body[idx..];
    let close = rest.find('>').expect("anchor close");
    let opening = &rest[..close];
    assert!(
        opening.contains("text-slate-400"),
        "expected greyed class on muted DM anchor: {opening}"
    );
}

#[tokio::test]
async fn sidebar_unread_badge_hidden_for_muted_dm() {
    let t = app_with_two_users("viewer", "alice").await;
    let dm_id = seed_dm_room(&t).await;

    // Peer sends 3 DM messages. Without mute, the badge would render `>3<`.
    for i in 0..3 {
        sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
            .bind(dm_id)
            .bind(&t.peer_id)
            .bind(format!("hi {i}"))
            .execute(&t.chat)
            .await
            .unwrap();
    }
    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );

    let (status, body) = get_root(&t.app, &t.session).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(r#"<span id="unread-dm-{}"></span>"#, t.peer_id)),
        "expected empty unread span for muted DM, got: {body}"
    );
}

#[tokio::test]
async fn dm_header_checkbox_reflects_persisted_state() {
    let t = app_with_two_users("viewer", "alice").await;
    seed_dm_room(&t).await;

    // Default: unchecked.
    let (status, body) = get_dm_page(&t.app, &t.session, &t.peer_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"id="lc-dm-header""#),
        "DM header missing: {body}"
    );
    // Checkbox present, NOT checked.
    let header_idx = body.find(r#"id="lc-dm-header""#).unwrap();
    let header_end = body[header_idx..]
        .find("</header>")
        .map(|e| header_idx + e)
        .unwrap_or(body.len());
    let header_html = &body[header_idx..header_end];
    assert!(header_html.contains(r#"name="muted""#));
    assert!(
        !header_html.contains("checked"),
        "default DM header should not have checked checkbox: {header_html}"
    );

    // Mute and reload: checked.
    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );
    let (_, body) = get_dm_page(&t.app, &t.session, &t.peer_id).await;
    let header_idx = body.find(r#"id="lc-dm-header""#).unwrap();
    let header_end = body[header_idx..]
        .find("</header>")
        .map(|e| header_idx + e)
        .unwrap_or(body.len());
    let header_html = &body[header_idx..header_end];
    assert!(
        header_html.contains("checked"),
        "muted DM header should have checked checkbox: {header_html}"
    );
}

#[tokio::test]
async fn upserting_dm_mute_overwrites_prior() {
    let t = app_with_two_users("viewer", "alice").await;
    let dm_id = seed_dm_room(&t).await;

    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on")
            .await
            .0,
        StatusCode::OK
    );
    let n = count_mute_rows(&t.chat, &t.viewer_id, dm_id).await;
    assert_eq!(n, 1, "second mute toggle should not duplicate the row");
}

#[tokio::test]
async fn block_does_not_break_existing_dm_mute_endpoint() {
    // Block is a separate concern; once a DM exists, the muter can still
    // toggle mute even if the conversation has since been blocked. This
    // pins the documented design choice in routes/dm_mute.rs.
    let t = app_with_two_users("viewer", "alice").await;
    seed_dm_room(&t).await;

    db::auth::block_user(&t.auth, &t.viewer_id, &t.peer_id)
        .await
        .unwrap();

    // The mute endpoint still succeeds (mute is a private per-user setting).
    let (status, _) = post_mute_full(&t.app, &t.session, &t.peer_id, "muted=on").await;
    assert_eq!(status, StatusCode::OK);
}
