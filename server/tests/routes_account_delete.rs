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
        geoip: None,
        login_approval_enabled: false,
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
    // LC-62: a scheduled message authored by the user must also be
    // wiped, regardless of state (pending / delivered / dropped).
    let sched_id = db::scheduled::insert_scheduled(
        &t.chat,
        room_id,
        &t.user_id,
        "scheduled-before-delete",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
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
    // LC-62: scheduled row authored by the deleted user is gone.
    assert!(
        db::scheduled::get_scheduled(&t.chat, sched_id)
            .await
            .unwrap()
            .is_none(),
        "scheduled row must be wiped with the user"
    );
}

// LC-22 cutover: the wrong-password test is gone. Account delete is now
// session-cookie + confirm-phrase only; Bunyip owns credentials and there is
// no password to re-confirm.

#[tokio::test]
async fn delete_rejects_wrong_phrase() {
    let t = app_with_user().await;
    let (status, _, headers) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "yes please delete")).await;
    // LC-356: a wrong phrase now flashes inline on /settings, not a full-page 400.
    assert!(status.is_redirection(), "expected redirect, got {status}");
    let loc = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(loc.starts_with("/settings?error="), "loc: {loc}");
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

    let (status, _body, headers) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    // LC-356: the sole-owner blocker now flashes inline on /settings; the
    // blocking enclave name rides the error query param.
    assert!(status.is_redirection(), "expected redirect, got {status}");
    let loc = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(loc.starts_with("/settings?error="), "loc: {loc}");
    assert!(loc.contains("my-club"), "loc: {loc}");

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

    let (status, _body, headers) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    // LC-356: the sole-admin blocker now flashes inline on /settings.
    assert!(status.is_redirection(), "expected redirect, got {status}");
    let loc = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(loc.starts_with("/settings?error="), "loc: {loc}");
    assert!(loc.to_lowercase().contains("only+admin"), "loc: {loc}");

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

// LC-908: `purge_user_chat` originally covered 17 tables by name and relied
// on cascade for the rest. The chat schema has since grown past that; this
// seeds every table the audit found uncovered and asserts none of them keep
// a row (or a dangling actor reference) for the deleted user.
#[tokio::test]
async fn delete_wipes_lc908_gap_tables() {
    let t = app_with_user().await;

    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let room_id =
        db::chat::create_room(&t.chat, "gap-room", None, "public", None, Some(general_id))
            .await
            .unwrap();
    // A message authored by someone else, so the user's footprint on it
    // (vote, ack, report, tag override) has no cascade path.
    let other_msg = db::chat::insert_message(&t.chat, room_id, &t.peer_id, "someone else's poll")
        .await
        .unwrap();

    // poll_votes: user votes on someone else's poll.
    sqlx::query("INSERT INTO polls (message_id, question) VALUES (?, 'q?')")
        .bind(other_msg)
        .execute(&t.chat)
        .await
        .unwrap();
    let option_id: i64 = sqlx::query_scalar(
        "INSERT INTO poll_options (message_id, position, text) VALUES (?, 0, 'yes') RETURNING id",
    )
    .bind(other_msg)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    sqlx::query("INSERT INTO poll_votes (option_id, user_id) VALUES (?, ?)")
        .bind(option_id)
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // saved_searches
    sqlx::query("INSERT INTO saved_searches (user_id, query) VALUES (?, 'from:bob')")
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // thread_followers / thread_muters, keyed off the other user's message.
    sqlx::query("INSERT INTO thread_followers (user_id, parent_id, room_id) VALUES (?, ?, ?)")
        .bind(&t.user_id)
        .bind(other_msg)
        .bind(room_id)
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query("INSERT INTO thread_muters (user_id, parent_id, room_id) VALUES (?, ?, ?)")
        .bind(&t.user_id)
        .bind(other_msg)
        .bind(room_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // kudos: given and received.
    sqlx::query("INSERT INTO kudos (giver_id, receiver_id, room_id) VALUES (?, ?, ?)")
        .bind(&t.user_id)
        .bind(&t.peer_id)
        .bind(room_id)
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query("INSERT INTO kudos (giver_id, receiver_id, room_id) VALUES (?, ?, ?)")
        .bind(&t.peer_id)
        .bind(&t.user_id)
        .bind(room_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // message_acks: user acknowledges someone else's message.
    sqlx::query("INSERT INTO message_acks (message_id, user_id) VALUES (?, ?)")
        .bind(other_msg)
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // canned_responses
    sqlx::query(
        "INSERT INTO canned_responses (user_id, name, body) VALUES (?, 'hi', 'hello there')",
    )
    .bind(&t.user_id)
    .execute(&t.chat)
    .await
    .unwrap();

    // room_role_overrides: the user holds a grant, and separately issued
    // one to the peer that must survive with the issuer reference cleared.
    sqlx::query(
        "INSERT INTO room_role_overrides (room_id, user_id, role, assigned_by) \
         VALUES (?, ?, 'moderator', ?)",
    )
    .bind(room_id)
    .bind(&t.user_id)
    .bind(&t.peer_id)
    .execute(&t.chat)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_role_overrides (room_id, user_id, role, assigned_by) \
         VALUES (?, ?, 'moderator', ?)",
    )
    .bind(room_id)
    .bind(&t.peer_id)
    .bind(&t.user_id)
    .execute(&t.chat)
    .await
    .unwrap();

    // room_nicknames
    sqlx::query("INSERT INTO room_nicknames (room_id, user_id, nickname) VALUES (?, ?, 'Al')")
        .bind(room_id)
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // user_group_members
    let group_id: i64 = sqlx::query_scalar(
        "INSERT INTO user_groups (enclave_id, name, created_by) VALUES (?, 'friends', ?) \
         RETURNING id",
    )
    .bind(general_id)
    .bind(&t.peer_id)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_group_members (group_id, user_id) VALUES (?, ?)")
        .bind(group_id)
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // message_reports: the user reports someone else's message, and
    // separately handled a report the peer filed.
    sqlx::query(
        "INSERT INTO message_reports (message_id, room_id, reporter_id, category) \
         VALUES (?, ?, ?, 'spam')",
    )
    .bind(other_msg)
    .bind(room_id)
    .bind(&t.user_id)
    .execute(&t.chat)
    .await
    .unwrap();
    // Anchored on a peer-authored message (not the user's own) so this row
    // survives the top-of-transaction `messages` delete and actually
    // exercises the new `handled_by` clear below, rather than cascading
    // away for free.
    let peer_msg_2 = db::chat::insert_message(&t.chat, room_id, &t.peer_id, "flag me")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO message_reports (message_id, room_id, reporter_id, handled_by, category) \
         VALUES (?, ?, ?, ?, 'spam')",
    )
    .bind(peer_msg_2)
    .bind(room_id)
    .bind(&t.peer_id)
    .bind(&t.user_id)
    .execute(&t.chat)
    .await
    .unwrap();

    // followups / followup_items: the user created one list (anchored on a
    // peer message so it survives the top-of-transaction messages delete),
    // and self-claimed + closed out an item on someone else's list.
    sqlx::query("INSERT INTO followups (message_id, created_by) VALUES (?, ?)")
        .bind(peer_msg_2)
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query("INSERT INTO followups (message_id, created_by) VALUES (?, ?)")
        .bind(other_msg)
        .bind(&t.peer_id)
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO followup_items (message_id, position, text, assignee_id, done, done_by) \
         VALUES (?, 0, 'do the thing', ?, 1, ?)",
    )
    .bind(other_msg)
    .bind(&t.user_id)
    .bind(&t.user_id)
    .execute(&t.chat)
    .await
    .unwrap();

    // user_storage_quotas
    sqlx::query("INSERT INTO user_storage_quotas (user_id, quota_bytes) VALUES (?, 1000)")
        .bind(&t.user_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // message_tag_overrides: the user moderated someone else's message.
    sqlx::query(
        "INSERT INTO message_tag_overrides (message_id, hidden, actor_user) VALUES (?, 1, ?)",
    )
    .bind(other_msg)
    .bind(&t.user_id)
    .execute(&t.chat)
    .await
    .unwrap();

    // enclave_last_room
    sqlx::query("INSERT INTO enclave_last_room (user_id, enclave_id, room_id) VALUES (?, ?, ?)")
        .bind(&t.user_id)
        .bind(general_id)
        .bind(room_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // dm_pairs (LC-947): the user has a DM room with the peer.
    db::chat::create_dm_room(&t.chat, "dm", &t.user_id, &t.peer_id)
        .await
        .unwrap();

    let (status, body, _) =
        post_delete(&t.app, &t.session, &form(PASSWORD, "delete my account")).await;
    assert!(
        status.is_redirection(),
        "expected redirect, got {status}: {}",
        String::from_utf8_lossy(&body)
    );

    async fn count(pool: &SqlitePool, sql: &str, id: &str) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    for (label, sql) in [
        (
            "poll_votes",
            "SELECT COUNT(*) FROM poll_votes WHERE user_id = ?",
        ),
        (
            "saved_searches",
            "SELECT COUNT(*) FROM saved_searches WHERE user_id = ?",
        ),
        (
            "thread_followers",
            "SELECT COUNT(*) FROM thread_followers WHERE user_id = ?",
        ),
        (
            "thread_muters",
            "SELECT COUNT(*) FROM thread_muters WHERE user_id = ?",
        ),
        (
            "kudos (giver)",
            "SELECT COUNT(*) FROM kudos WHERE giver_id = ?",
        ),
        (
            "kudos (receiver)",
            "SELECT COUNT(*) FROM kudos WHERE receiver_id = ?",
        ),
        (
            "message_acks",
            "SELECT COUNT(*) FROM message_acks WHERE user_id = ?",
        ),
        (
            "canned_responses",
            "SELECT COUNT(*) FROM canned_responses WHERE user_id = ?",
        ),
        (
            "room_role_overrides (user)",
            "SELECT COUNT(*) FROM room_role_overrides WHERE user_id = ?",
        ),
        (
            "room_role_overrides (assigned_by)",
            "SELECT COUNT(*) FROM room_role_overrides WHERE assigned_by = ?",
        ),
        (
            "room_nicknames",
            "SELECT COUNT(*) FROM room_nicknames WHERE user_id = ?",
        ),
        (
            "user_group_members",
            "SELECT COUNT(*) FROM user_group_members WHERE user_id = ?",
        ),
        (
            "message_reports (reporter)",
            "SELECT COUNT(*) FROM message_reports WHERE reporter_id = ?",
        ),
        (
            "message_reports (handled_by)",
            "SELECT COUNT(*) FROM message_reports WHERE handled_by = ?",
        ),
        (
            "followups",
            "SELECT COUNT(*) FROM followups WHERE created_by = ?",
        ),
        (
            "followup_items (assignee)",
            "SELECT COUNT(*) FROM followup_items WHERE assignee_id = ?",
        ),
        (
            "followup_items (done_by)",
            "SELECT COUNT(*) FROM followup_items WHERE done_by = ?",
        ),
        (
            "user_storage_quotas",
            "SELECT COUNT(*) FROM user_storage_quotas WHERE user_id = ?",
        ),
        (
            "message_tag_overrides",
            "SELECT COUNT(*) FROM message_tag_overrides WHERE actor_user = ?",
        ),
        (
            "enclave_last_room",
            "SELECT COUNT(*) FROM enclave_last_room WHERE user_id = ?",
        ),
        (
            "dm_pairs (user_lo)",
            "SELECT COUNT(*) FROM dm_pairs WHERE user_lo = ?",
        ),
        (
            "dm_pairs (user_hi)",
            "SELECT COUNT(*) FROM dm_pairs WHERE user_hi = ?",
        ),
    ] {
        assert_eq!(
            count(&t.chat, sql, &t.user_id).await,
            0,
            "{label} kept a row for the deleted user"
        );
    }

    // The peer's grant survives; only the issuer reference was cleared.
    let peer_override_by: String =
        sqlx::query_scalar("SELECT assigned_by FROM room_role_overrides WHERE user_id = ?")
            .bind(&t.peer_id)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(peer_override_by, "");

    // The peer's followup list survives with the item unassigned and
    // its completion actor cleared, not deleted outright.
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM followup_items WHERE message_id = ?")
        .bind(other_msg)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(
        items, 1,
        "peer's followup item should survive, just unassigned"
    );
}

// LC-908: schema-walking guard. `purge_user_chat` enumerates tables by name,
// which silently goes stale as `chat.db` grows (that staleness is exactly
// how this issue's 16-table gap happened). This walks the live schema for
// every column shaped like a user reference and requires each one to be
// either wiped/neutralised by `purge_user_chat` (see the matching entries in
// `PURGED_COLUMNS`, one per `sqlx::query` there) or explicitly annotated in
// `RETAINED_COLUMNS` with why it is allowed to keep a deleted user's id.
// Adding a new table with a matching column and no entry in either list
// fails this test.
#[tokio::test]
async fn chat_user_columns_are_purged_or_allowlisted() {
    let chat = common::pool("chat").await;

    // (table, column) pairs `purge_user_chat` deletes outright or clears to
    // NULL / empty. Kept in the same order as the statements in
    // `server/src/routes/account.rs` for easy cross-checking.
    const PURGED_COLUMNS: &[(&str, &str)] = &[
        ("messages", "user_id"),
        ("message_reactions", "user_id"),
        ("bookmarks", "user_id"),
        ("pinned_messages", "pinned_by"),
        ("scheduled_messages", "user_id"),
        ("reminders", "user_id"),
        ("message_drafts", "user_id"),
        ("custom_emojis", "uploaded_by"),
        // Personal-scoped rows always have user_id == uploaded_by (see
        // `db::custom_emojis::insert_for_user`), so the uploaded_by delete
        // above removes them too.
        ("custom_emojis", "user_id"),
        ("room_members", "user_id"),
        ("room_notification_settings", "user_id"),
        ("dm_read_state", "user_id"),
        ("enclave_members", "user_id"),
        ("enclave_invitations", "invited_by"),
        ("mod_actions", "target_user"),
        ("poll_votes", "user_id"),
        ("saved_searches", "user_id"),
        ("thread_followers", "user_id"),
        ("thread_muters", "user_id"),
        ("kudos", "giver_id"),
        ("kudos", "receiver_id"),
        ("message_acks", "user_id"),
        ("canned_responses", "user_id"),
        ("room_role_overrides", "user_id"),
        ("room_role_overrides", "assigned_by"),
        ("room_nicknames", "user_id"),
        ("user_group_members", "user_id"),
        ("message_reports", "reporter_id"),
        ("message_reports", "handled_by"),
        ("followups", "created_by"),
        ("followup_items", "assignee_id"),
        ("followup_items", "done_by"),
        ("user_storage_quotas", "user_id"),
        ("message_tag_overrides", "actor_user"),
        ("enclave_last_room", "user_id"),
        ("dm_pairs", "user_lo"),
        ("dm_pairs", "user_hi"),
    ];

    // (table, column) pairs intentionally left holding a user id after
    // account delete, with the reason. These are resource/attribution or
    // audit-trail columns describing who created or actioned a durable row,
    // not the user's own profile or content footprint.
    const RETAINED_COLUMNS: &[(&str, &str)] = &[
        ("branding", "updated_by"),
        ("bridges", "created_by"),
        ("call_transcripts", "started_by"),
        ("email_inboxes", "created_by"),
        ("enclaves", "created_by"),
        ("incoming_webhooks", "created_by"),
        ("link_filter_quarantine", "reviewed_by"),
        ("link_filter_rules", "created_by"),
        ("message_ack_required", "required_by"),
        ("messages", "deleted_by"),
        // Pre-existing convention (see the comment in purge_user_chat):
        // the actor of a moderation action stays on the audit trail.
        ("mod_actions", "actor_user"),
        ("outgoing_webhooks", "created_by"),
        ("room_automations", "created_by"),
        ("room_feeds", "created_by"),
        ("rooms", "created_by"),
        ("rooms", "wiki_updated_by"),
        ("slash_commands_custom", "created_by"),
        ("support_tickets", "handled_by"),
        ("transcript_segments", "user_id"),
        ("user_groups", "created_by"),
        ("voice_events", "user_id"),
        // LC-908 audit follow-up, deferred as LC-923: these two are
        // genuinely user-keyed and not yet purged.
        ("enclave_bans", "user_id"),
        ("reply_tokens", "user_id"),
    ];

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )
    .fetch_all(&chat)
    .await
    .unwrap();

    let is_user_shaped = |col: &str| {
        col == "user_id"
            || col.ends_with("_by")
            || col.ends_with("_user")
            || col == "giver_id"
            || col == "receiver_id"
            || col == "reporter_id"
            || col == "assignee_id"
            || col == "user_lo"
            || col == "user_hi"
    };

    let mut uncovered = Vec::new();
    for table in &tables {
        let cols: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .fetch_all(&chat)
                .await
                .unwrap();
        for col in cols {
            if !is_user_shaped(&col) {
                continue;
            }
            let pair = (table.as_str(), col.as_str());
            if !PURGED_COLUMNS.contains(&pair) && !RETAINED_COLUMNS.contains(&pair) {
                uncovered.push(format!("{table}.{col}"));
            }
        }
    }

    assert!(
        uncovered.is_empty(),
        "chat.db has user-id column(s) not covered by purge_user_chat or an \
         annotated allowlist entry: {uncovered:?}"
    );
}
