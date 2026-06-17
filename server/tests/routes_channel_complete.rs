//! LC-323: the composer's `#channel` autocomplete endpoint.
//! GET /rooms/{id}/channel-complete?q= returns a listbox of accessible
//! same-enclave rooms matching the prefix, access-gated to the room. Mirrors
//! the routes_emoji_complete harness (admin promote + backfill + totp_enabled).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-channel-complete-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

async fn complete(app: &Router, sess: &str, room_id: i64, q: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/rooms/{room_id}/channel-complete?q={q}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

struct Setup {
    app: Router,
    admin_session: String,
    member_session: String,
    outsider_session: String,
    general_room: i64,
    private_room: i64,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    let outsider_id = db::auth::create_user(&auth, "outsider", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id IN (?, ?)")
        .bind(&member_id)
        .bind(&outsider_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // Room 1 is the General room; seed more public rooms in that enclave so a
    // prefix query can match. One has a space in its name to confirm the
    // linkable-charset filter drops it.
    let general_enclave: i64 = sqlx::query_scalar("SELECT enclave_id FROM rooms WHERE id=1")
        .fetch_one(&chat)
        .await
        .unwrap();
    // Names chosen to not collide with the migration-seeded default rooms
    // ("general", "random"). "spaced name" exercises the linkable-charset
    // filter.
    for name in ["devops", "support", "spaced name"] {
        db::chat::create_room(&chat, name, None, "public", None, Some(general_enclave))
            .await
            .unwrap();
    }

    // A private room (no enclave) the outsider is not a member of.
    let private_room = db::chat::create_room(&chat, "secret", None, "private", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, private_room, &admin_id)
        .await
        .unwrap();

    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    let outsider_session = db::auth::create_session(&auth, &outsider_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
    };
    Setup {
        app: routes::build_router(state),
        admin_session,
        member_session,
        outsider_session,
        general_room: 1,
        private_room,
    }
}

#[tokio::test]
async fn prefix_matches_same_enclave_room() {
    let s = setup().await;
    let (status, body) = complete(&s.app, &s.member_session, s.general_room, "dev").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("data-insert=\"#devops\""),
        "devops not offered: {body}"
    );
    assert!(
        !body.contains("data-insert=\"#support\""),
        "support should not match prefix 'dev': {body}"
    );
}

#[tokio::test]
async fn empty_query_lists_enclave_rooms() {
    let s = setup().await;
    let (status, body) = complete(&s.app, &s.member_session, s.general_room, "").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("data-insert=\"#devops\""),
        "devops missing: {body}"
    );
    assert!(
        body.contains("data-insert=\"#support\""),
        "support missing: {body}"
    );
}

#[tokio::test]
async fn non_linkable_name_is_filtered_out() {
    let s = setup().await;
    // "spaced name" contains a space, so it can never be a #token and must not
    // be offered (autocomplete and render stay consistent).
    let (status, body) = complete(&s.app, &s.member_session, s.general_room, "").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        !body.contains("spaced name"),
        "non-linkable name offered: {body}"
    );
}

#[tokio::test]
async fn non_member_is_forbidden() {
    let s = setup().await;
    let (status, _) = complete(&s.app, &s.outsider_session, s.private_room, "ra").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_enclave_room_yields_no_suggestions() {
    let s = setup().await;
    // The private "secret" room has no enclave, so there are no #channel targets
    // even for an accessible viewer (the admin is a member). An empty list still
    // returns 200 with the hidden placeholder ul, never an option row.
    let (status, body) = complete(&s.app, &s.admin_session, s.private_room, "").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        !body.contains("data-insert="),
        "non-enclave room offered suggestions: {body}"
    );
}
