use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p =
                std::env::temp_dir().join(format!("lc-edit-history-tests-{}", std::process::id()));
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

struct TestApp {
    app: Router,
    viewer_session: String,
    peer_session: String,
    viewer_id: String,
    peer_id: String,
    chat: SqlitePool,
}

async fn app_with_two_users(viewer: &str, peer: &str) -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, viewer, "hash").await.unwrap();
    let peer_id = db::auth::create_user(&auth, peer, "hash").await.unwrap();
    // Promote viewer to admin so backfill_general_membership can run (it
    // early-returns when no admin exists, which would leave both users
    // outside the General enclave and 403 every post to room 1). Alice
    // stays at the schema default 'user' role so the private-room access
    // check exercises member-list logic for her, not admin god-mode. Both
    // get totp_enabled=1 so the enforce_2fa_enrollment middleware does not
    // redirect every authed request to /settings/2fa/setup; AppState below
    // sets secret_key which activates that middleware. Matches the
    // workaround in routes_mentions.rs and routes_broadcast_mentions.rs.
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&peer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    let peer_session = db::auth::create_session(&auth, &peer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
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
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        viewer_session,
        peer_session,
        viewer_id,
        peer_id,
        chat: chat_for_test,
    }
}

fn form_encode(body: &str) -> String {
    assert!(
        body.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b' ' | b'-' | b'_' | b'*')),
        "form_encode helper does not handle char in {body:?}"
    );
    body.replace(' ', "+")
}

async fn post_message(app: &Router, sess: &str, room_id: i64, body: &str) -> StatusCode {
    let form = format!("body={}&file_id=", form_encode(body));
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn patch_message(app: &Router, sess: &str, message_id: i64, body: &str) -> StatusCode {
    let form = format!("body={}", form_encode(body));
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/messages/{message_id}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn get_history(app: &Router, sess: Option<&str>, message_id: i64) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(format!("/messages/{message_id}/history"));
    if let Some(s) = sess {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    let req = builder.body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes).to_string();
    (status, body)
}

async fn last_message_id(chat: &SqlitePool, room_id: i64) -> i64 {
    let row = sqlx::query("SELECT id FROM messages WHERE room_id = ? ORDER BY id DESC LIMIT 1")
        .bind(room_id)
        .fetch_one(chat)
        .await
        .unwrap();
    row.get::<i64, _>("id")
}

#[tokio::test]
async fn unedited_message_returns_single_current_entry() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "hello world").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hello world"), "body: {body}");
    assert_eq!(
        body.matches("hello world").count(),
        1,
        "single Current entry expected: {body}"
    );
}

#[tokio::test]
async fn unedited_message_label_has_no_trailing_timestamp() {
    // Regression guard: m.edited_at = None must render as "Current", not
    // "Current - last edited <empty>". A future refactor that resurrects
    // unwrap_or_default() on edited_at would re-introduce the misleading
    // trailing-space label.
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "fresh post").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Current"),
        "body must include 'Current' label: {body}"
    );
    assert!(
        !body.contains("Current - last edited"),
        "unedited body must not contain 'Current - last edited': {body}"
    );
}

#[tokio::test]
async fn two_edits_render_three_entries_oldest_first() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "v0").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "v1").await,
        StatusCode::OK
    );
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "v2").await,
        StatusCode::OK
    );

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("v0"), "body: {body}");
    assert!(body.contains("v1"), "body: {body}");
    assert!(body.contains("v2"), "body: {body}");
    let p0 = body.find("v0").unwrap();
    let p1 = body.find("v1").unwrap();
    let p2 = body.find("v2").unwrap();
    assert!(p0 < p1 && p1 < p2, "ordering: v0={p0} v1={p1} v2={p2}");
    // Current entry carries the "last edited <ts>" suffix because the
    // message has been edited.
    assert!(
        body.contains("Current - last edited"),
        "edited message must show 'Current - last edited' label: {body}"
    );
}

#[tokio::test]
async fn unauthenticated_request_redirects_or_unauthorized() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "hi").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status, _) = get_history(&t.app, None, mid).await;
    assert!(
        status == StatusCode::SEE_OTHER
            || status == StatusCode::TEMPORARY_REDIRECT
            || status == StatusCode::FOUND
            || status == StatusCode::UNAUTHORIZED,
        "status: {status}"
    );
}

#[tokio::test]
async fn non_room_member_blocked_from_private_history() {
    let t = app_with_two_users("viewer", "alice").await;
    // Private room owned by viewer; alice is not a member and not an admin.
    let private_id: i64 = sqlx::query_scalar(
        "INSERT INTO rooms (name, room_type, enclave_id) \
         SELECT 'private-edit', 'private', enclave_id FROM rooms WHERE id = 1 \
         RETURNING id",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    sqlx::query("INSERT INTO room_members (room_id, user_id) VALUES (?, ?)")
        .bind(private_id)
        .bind(&t.viewer_id)
        .execute(&t.chat)
        .await
        .unwrap();
    assert_eq!(
        post_message(&t.app, &t.viewer_session, private_id, "secret").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, private_id).await;
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "secret v2").await,
        StatusCode::OK
    );

    let (status, _) = get_history(&t.app, Some(&t.peer_session), mid).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "alice should be blocked from a private room she's not a member of"
    );
}

#[tokio::test]
async fn soft_deleted_message_history_returns_404() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "doomed").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "still doomed").await,
        StatusCode::OK
    );
    sqlx::query("UPDATE messages SET deleted_at = datetime('now'), deleted_by = ? WHERE id = ?")
        .bind(&t.viewer_id)
        .bind(mid)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, _) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn markdown_in_prior_body_renders_through_pipeline() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "**emphasis** present").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "rewritten plain").await,
        StatusCode::OK
    );

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<strong>emphasis</strong>"),
        "prior body markdown must render: {body}"
    );
}

#[tokio::test]
async fn mention_chip_persists_when_removed_by_edit() {
    // Load-bearing test for mentions_for_body's existence: prior body has
    // @alice, current body removed her, the drawer still shows her chip on
    // the prior entry. Live-path mentions_for_messages would miss this
    // case because reconcile_mentions deletes her row when the edit lands.
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "hi @alice").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "hi everyone").await,
        StatusCode::OK
    );

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    let alice_profile = format!("/profile/{}", t.peer_id);
    assert!(
        body.contains(&alice_profile),
        "prior body should render a chip linking to alice's profile: {body}"
    );
    assert!(
        body.contains(">@alice</a>"),
        "chip label missing on prior body: {body}"
    );
}

#[tokio::test]
async fn mention_chip_overlap_renders_in_both_entries() {
    // Mention persisted across the edit appears as a chip in both the
    // prior body and the current body.
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "ping @alice round 1").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "ping @alice round 2").await,
        StatusCode::OK
    );

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    let alice_profile = format!("/profile/{}", t.peer_id);
    let chip_count = body.matches(&alice_profile).count();
    assert_eq!(
        chip_count, 2,
        "expected one chip per entry (prior + current): {body}"
    );
}

#[tokio::test]
async fn unresolved_mention_token_renders_as_literal_text() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "shout out @nosuchuser").await,
        StatusCode::OK
    );
    let mid = last_message_id(&t.chat, 1).await;
    // Edit so the message has a prior version archived.
    assert_eq!(
        patch_message(&t.app, &t.viewer_session, mid, "never mind").await,
        StatusCode::OK
    );

    let (status, body) = get_history(&t.app, Some(&t.viewer_session), mid).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("@nosuchuser"),
        "prior body must show literal token: {body}"
    );
    assert!(
        !body.contains(">@nosuchuser</a>"),
        "unresolved token must not render as a chip anchor: {body}"
    );
}
