//! LC-339/LC-341: Coyote Mode coverage - the manager-gated toggle, enclave-ban
//! enforcement (a banned user cannot post or rejoin), and the auto-ban TRIGGER.
//! The trigger fires from a fire-and-forget spawn in `post_message`; LC-341
//! extracted the gated decision+action into `routes::maybe_coyote_ban`, which
//! these tests call directly (awaited) to assert it deterministically without
//! racing the spawn. The detection/purge SQL is unit-tested in `db_coyote_mode.rs`.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-coyote-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    member_id: String,
    admin_id: String,
    chat: SqlitePool,
    // LC-341: kept so trigger tests can call routes::maybe_coyote_ban directly.
    state: AppState,
}

/// Two users in the General enclave (id 1): `admin` (owner) and `member`
/// (regular member). Mirrors the routes_sidebar_categories harness.
async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&member_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
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
    let state_for_test = state.clone();
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        member_id,
        admin_id,
        chat: chat_for_test,
        state: state_for_test,
    }
}

/// Build a `User` for `user_id` from the auth DB (for direct
/// `maybe_coyote_ban` calls).
async fn load_user(t: &TestApp, user_id: &str) -> lets_chat::models::User {
    let rec = db::auth::find_user_by_id(&t.state.auth, user_id)
        .await
        .unwrap()
        .unwrap();
    lets_chat::models::User::from(rec)
}

/// Create `n` fresh public rooms in enclave 1 and return their ids - the
/// distinct rooms a burst test posts into.
async fn enclave_rooms(t: &TestApp, n: usize) -> Vec<i64> {
    let mut ids: Vec<i64> = Vec::new();
    for i in 0..n {
        ids.push(
            db::chat::create_room(
                &t.state.chat,
                &format!("burst{i}"),
                None,
                "public",
                None,
                Some(1),
            )
            .await
            .unwrap(),
        );
    }
    ids
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn member_cannot_toggle_coyote_mode() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/enclave/1/coyote-mode",
        "enabled=1",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let e = db::enclave::get_enclave(&t.chat, 1).await.unwrap().unwrap();
    assert!(!e.coyote_mode, "non-manager toggle must not take effect");
}

#[tokio::test]
async fn manager_toggles_coyote_mode() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/enclave/1/coyote-mode",
        "enabled=1",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let e = db::enclave::get_enclave(&t.chat, 1).await.unwrap().unwrap();
    assert!(e.coyote_mode);

    // Toggle back off.
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/enclave/1/coyote-mode",
        "enabled=0",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let e = db::enclave::get_enclave(&t.chat, 1).await.unwrap().unwrap();
    assert!(!e.coyote_mode);
}

#[tokio::test]
async fn enclave_ban_blocks_posting() {
    let t = app().await;
    // Sanity: an un-banned member can post in the enclave's room 1.
    let ok = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/messages",
        "body=hello",
    )
    .await;
    assert_eq!(ok, StatusCode::NO_CONTENT, "member posts before ban");

    db::enclave::ban_from_enclave(&t.chat, 1, &t.member_id, "test")
        .await
        .unwrap();

    let blocked = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/messages",
        "body=again",
    )
    .await;
    assert_eq!(blocked, StatusCode::FORBIDDEN, "banned member cannot post");
}

#[tokio::test]
async fn member_cannot_unban() {
    let t = app().await;
    db::enclave::ban_from_enclave(&t.chat, 1, &t.member_id, "test")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/enclave/1/bans/{}/unban", t.member_id),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        db::enclave::is_enclave_banned(&t.chat, 1, &t.member_id)
            .await
            .unwrap(),
        "non-manager unban must not take effect"
    );
}

#[tokio::test]
async fn manager_unban_then_member_can_rejoin_and_post() {
    let t = app().await;
    db::enclave::set_public(&t.chat, 1, true).await.unwrap();
    db::enclave::ban_from_enclave(&t.chat, 1, &t.member_id, "test")
        .await
        .unwrap();
    // Banned -> rejoin refused.
    let rejoin_blocked = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/enclaves/discover/1/join",
        "",
    )
    .await;
    assert_eq!(rejoin_blocked, StatusCode::FORBIDDEN);

    // Manager lifts the ban.
    let unban = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/enclave/1/bans/{}/unban", t.member_id),
        "",
    )
    .await;
    assert_eq!(unban, StatusCode::SEE_OTHER);
    assert!(!db::enclave::is_enclave_banned(&t.chat, 1, &t.member_id)
        .await
        .unwrap());

    // Unbanned -> can rejoin (regains membership), then post.
    let rejoin = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/enclaves/discover/1/join",
        "",
    )
    .await;
    assert_eq!(rejoin, StatusCode::SEE_OTHER, "unbanned member rejoins");
    let post = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/messages",
        "body=back",
    )
    .await;
    assert_eq!(post, StatusCode::NO_CONTENT, "rejoined member can post");
}

#[tokio::test]
async fn enclave_ban_blocks_rejoin() {
    let t = app().await;
    db::enclave::set_public(&t.chat, 1, true).await.unwrap();
    db::enclave::ban_from_enclave(&t.chat, 1, &t.member_id, "test")
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/enclaves/discover/1/join",
        "",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "banned member cannot rejoin via discover"
    );
}

// LC-341: deterministic coverage of the auto-ban TRIGGER. The production path
// fires maybe_coyote_ban from a fire-and-forget spawn; these call the extracted
// fn directly (awaited) so the assertions never race the spawn.

async fn post_in(t: &TestApp, rooms: &[i64], user_id: &str) {
    for r in rooms {
        db::chat::insert_message(&t.chat, *r, user_id, "spam")
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn coyote_trigger_bans_on_three_room_burst() {
    let t = app().await;
    db::enclave::set_coyote_mode(&t.chat, 1, true)
        .await
        .unwrap();
    let rooms = enclave_rooms(&t, 3).await;
    post_in(&t, &rooms, &t.member_id).await;
    let member = load_user(&t, &t.member_id).await;

    let fired = lets_chat::routes::maybe_coyote_ban(&t.state, 1, rooms[0], &member)
        .await
        .unwrap();
    assert!(fired, "3 distinct rooms in window must trigger");
    assert!(db::enclave::is_enclave_banned(&t.chat, 1, &t.member_id)
        .await
        .unwrap());
    let live: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE user_id=? AND deleted_at IS NULL")
            .bind(&t.member_id)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(live, 0, "burst messages soft-deleted");
}

#[tokio::test]
async fn coyote_no_trigger_below_threshold() {
    let t = app().await;
    db::enclave::set_coyote_mode(&t.chat, 1, true)
        .await
        .unwrap();
    let rooms = enclave_rooms(&t, 2).await; // only 2 distinct rooms
    post_in(&t, &rooms, &t.member_id).await;
    let member = load_user(&t, &t.member_id).await;

    let fired = lets_chat::routes::maybe_coyote_ban(&t.state, 1, rooms[0], &member)
        .await
        .unwrap();
    assert!(!fired, "2 rooms is below the 3-room threshold");
    assert!(!db::enclave::is_enclave_banned(&t.chat, 1, &t.member_id)
        .await
        .unwrap());
}

#[tokio::test]
async fn coyote_off_no_trigger() {
    let t = app().await; // coyote_mode left off
    let rooms = enclave_rooms(&t, 3).await;
    post_in(&t, &rooms, &t.member_id).await;
    let member = load_user(&t, &t.member_id).await;

    let fired = lets_chat::routes::maybe_coyote_ban(&t.state, 1, rooms[0], &member)
        .await
        .unwrap();
    assert!(!fired, "mode off must never trigger");
    assert!(!db::enclave::is_enclave_banned(&t.chat, 1, &t.member_id)
        .await
        .unwrap());
}

#[tokio::test]
async fn coyote_exempts_admin() {
    let t = app().await;
    db::enclave::set_coyote_mode(&t.chat, 1, true)
        .await
        .unwrap();
    let rooms = enclave_rooms(&t, 3).await;
    post_in(&t, &rooms, &t.admin_id).await;
    let admin = load_user(&t, &t.admin_id).await; // site admin + enclave owner

    let fired = lets_chat::routes::maybe_coyote_ban(&t.state, 1, rooms[0], &admin)
        .await
        .unwrap();
    assert!(!fired, "managers / site admins are exempt");
    assert!(!db::enclave::is_enclave_banned(&t.chat, 1, &t.admin_id)
        .await
        .unwrap());
}
