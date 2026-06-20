//! LC-296: the composer's `:shortcode:` emoji autocomplete endpoint.
//! GET /rooms/{id}/emoji-complete?q= returns a listbox fragment of matching
//! emojis (custom first, then Unicode), access-gated to the room. Mirrors the
//! routes_reactions_authz harness (admin promote + backfill + totp_enabled).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-emoji-complete-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

async fn complete(app: &Router, sess: &str, room_id: i64, q: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/rooms/{room_id}/emoji-complete?q={q}"))
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

// LC-316: the composer emoji picker panel.
async fn picker(app: &Router, sess: &str, room_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/rooms/{room_id}/emoji-picker"))
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

    // Seed a custom emoji in the General enclave so a shortcode query can match
    // it. Room 1 is the General room created by the backfill above.
    let general_enclave: i64 = sqlx::query_scalar("SELECT enclave_id FROM rooms WHERE id=1")
        .fetch_one(&chat)
        .await
        .unwrap();
    db::custom_emojis::insert(
        &chat,
        general_enclave,
        "partyblob",
        "emojis/partyblob.png",
        "image/png",
        123,
        &admin_id,
    )
    .await
    .unwrap();

    // A private room the outsider is not a member of.
    let private_room = db::chat::create_room(&chat, "secret", None, "private", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, private_room, &admin_id)
        .await
        .unwrap();

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
        bunyip_sso: None,
    };
    Setup {
        app: routes::build_router(state),
        member_session,
        outsider_session,
        general_room: 1,
        private_room,
    }
}

#[tokio::test]
async fn unicode_match_returned_for_shortcode_prefix() {
    let s = setup().await;
    let (status, body) = complete(&s.app, &s.member_session, s.general_room, "tada").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // The "tada" shortcode maps to the party-popper glyph.
    assert!(body.contains("\u{1F389}"), "no tada glyph: {body}");
    assert!(body.contains("data-insert"), "no insert hook: {body}");
}

#[tokio::test]
async fn custom_emoji_matches_and_inserts_shortcode_token() {
    let s = setup().await;
    let (status, body) = complete(&s.app, &s.member_session, s.general_room, "party").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // Custom emoji rows insert the full :shortcode: token and preview the img.
    assert!(
        body.contains("data-insert=\":partyblob:\""),
        "custom emoji not offered: {body}"
    );
    assert!(
        body.contains("/api/emojis/"),
        "no custom emoji preview: {body}"
    );
}

#[tokio::test]
async fn empty_query_lists_custom_only() {
    let s = setup().await;
    let (status, body) = complete(&s.app, &s.member_session, s.general_room, "").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains(":partyblob:"),
        "custom not listed on bare colon: {body}"
    );
    // No Unicode rows when the query is empty (would be the full dump).
    assert!(
        !body.contains("data-insert=\"\u{1F389}\""),
        "unicode leaked on empty q: {body}"
    );
}

#[tokio::test]
async fn non_member_is_forbidden() {
    let s = setup().await;
    let (status, _) = complete(&s.app, &s.outsider_session, s.private_room, "tada").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn picker_lists_unicode_and_custom_with_insert_hooks() {
    let s = setup().await;
    let (status, body) = picker(&s.app, &s.member_session, s.general_room).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // LC-389: the picker now ships the full categorized Unicode set plus the
    // seeded custom emoji, each with an insert hook + a filter token; the filter
    // input is the LC-274 one. tada is one representative glyph from the set.
    assert!(
        body.contains("\u{1F389}"),
        "no tada glyph in full set: {body}"
    );
    assert!(
        body.contains("data-lc-emoji-insert=\":partyblob:\""),
        "custom emoji insert hook missing: {body}"
    );
    assert!(
        body.contains("data-lc-emoji-filter"),
        "no filter input: {body}"
    );
    assert!(
        body.contains("data-lc-emoji-name="),
        "no filter tokens: {body}"
    );
    // LC-389: categorized - the tab strip + at least the first section render,
    // and the Custom section appears because general_room's enclave has one.
    assert!(
        body.contains("data-lc-emoji-tabs"),
        "no category tab strip: {body}"
    );
    assert!(
        body.contains("data-lc-emoji-cat=\"smileys\""),
        "no smileys category section: {body}"
    );
    assert!(
        body.contains("data-lc-emoji-cat=\"custom\""),
        "no custom category section: {body}"
    );
}

#[tokio::test]
async fn picker_forbidden_for_non_member() {
    let s = setup().await;
    let (status, _) = picker(&s.app, &s.outsider_session, s.private_room).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
