#![cfg(feature = "standalone")]
//! LC-1096: POST /admin/rooms/{id}/invite is private-rooms-only on the server,
//! matching the enclave member routes (LC-1087/LC-1095). The template already
//! hid the invite form for non-private rooms, but a hand-crafted POST naming a
//! public room inserted a useless room_members row, and a missing room fell
//! through to the insert instead of a 404. Standalone-only because
//! `routes::admin` is `#[cfg(feature = "standalone")]`.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::common;

const SECRET: [u8; 32] = [31u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-room-invite-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    app: Router,
    admin_session: String,
    chat: SqlitePool,
    private_room: i64,
    public_room: i64,
    target_username: String,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();

    // The user we try to invite.
    db::auth::create_user(&auth, "target", "h").await.unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // A private and a public room in the General enclave (room 1).
    let private_room = db::chat::create_room(&chat, "priv", None, "private", None, Some(1))
        .await
        .unwrap();
    let public_room = db::chat::create_room(&chat, "pub", None, "public", None, Some(1))
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
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
        bg,
        secret_key: Some(Arc::new(SECRET)),
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
    Fixture {
        app: routes::build_router(state),
        admin_session,
        chat: chat_for_test,
        private_room,
        public_room,
        target_username: "target".to_string(),
    }
}

async fn invite(f: &Fixture, room_id: i64) -> (StatusCode, String) {
    let body = format!("username={}", f.target_username);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/admin/rooms/{room_id}/invite"))
        .header(header::COOKIE, format!("session={}", f.admin_session))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from(body))
        .unwrap();
    let res = f.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn member_count(f: &Fixture, room_id: i64) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_members WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(&f.chat)
        .await
        .unwrap()
}

#[tokio::test]
async fn invite_to_a_public_room_is_refused_and_adds_no_member() {
    let f = setup().await;
    let (st, body) = invite(&f, f.public_room).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "public room invite is 400: {body}"
    );
    assert!(
        body.contains("room is not private"),
        "the refusal names the reason: {body}"
    );
    assert_eq!(
        member_count(&f, f.public_room).await,
        0,
        "no room_members row was inserted for the public room"
    );
}

#[tokio::test]
async fn invite_to_a_missing_room_is_404() {
    let f = setup().await;
    let (st, _) = invite(&f, 999_999).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invite_to_a_private_room_still_adds_the_member() {
    let f = setup().await;
    assert_eq!(member_count(&f, f.private_room).await, 0, "precondition");
    let (st, _) = invite(&f, f.private_room).await;
    assert_eq!(st, StatusCode::OK, "private room invite still succeeds");
    assert_eq!(
        member_count(&f, f.private_room).await,
        1,
        "the member was inserted for the private room"
    );
}
