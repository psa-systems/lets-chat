//! LC-298: GET /users/{id}/hovercard returns the profile popover fragment.
//! Auth-gated; bio is shown for public profiles (or to self / admin) and hidden
//! otherwise; the Message link is gated on DM-permission. Mirrors the
//! routes_reactions_authz harness (admin promote + backfill + totp_enabled).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn hovercard(app: &Router, sess: Option<&str>, user_id: &str) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(format!("/users/{user_id}/hovercard"));
    if let Some(s) = sess {
        req = req.header(header::COOKIE, format!("session={s}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
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
    member_id: String,
    public_id: String,
    private_id: String,
}

async fn setup() -> Setup {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    let public_id = db::auth::create_user(&auth, "publicuser", "h")
        .await
        .unwrap();
    let private_id = db::auth::create_user(&auth, "privateuser", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id IN (?, ?, ?)")
        .bind(&member_id)
        .bind(&public_id)
        .bind(&private_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // publicuser: public profile + display name + bio. privateuser: private + bio.
    db::auth::set_profile_public(&auth, &public_id, true)
        .await
        .unwrap();
    db::auth::set_profile_public(&auth, &private_id, false)
        .await
        .unwrap();
    // LC-533: both carry the profile extras so the card + privacy-gate paths
    // can be asserted. New York is EDT (UTC-4) in July, London BST (UTC+1).
    sqlx::query(
        "UPDATE users SET display_name='Pub Lic', bio='hello from public bio', \
         pronouns='they/them', profile_links='https://pub.example', \
         timezone='America/New_York' WHERE id=?",
    )
    .bind(&public_id)
    .execute(&auth)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE users SET bio='secret private bio', pronouns='she/her', \
         profile_links='https://priv.example', timezone='Europe/London' WHERE id=?",
    )
    .bind(&private_id)
    .execute(&auth)
    .await
    .unwrap();

    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();

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
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    Setup {
        app: routes::build_router(state),
        admin_session,
        member_session,
        member_id,
        public_id,
        private_id,
    }
}

#[tokio::test]
async fn public_card_shows_name_username_and_bio() {
    let s = setup().await;
    let (status, body) = hovercard(&s.app, Some(&s.member_session), &s.public_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("Pub Lic"), "display name missing: {body}");
    assert!(body.contains("@publicuser"), "username missing: {body}");
    assert!(
        body.contains("hello from public bio"),
        "bio missing: {body}"
    );
    // LC-533: public profile extras are visible.
    assert!(body.contains("they/them"), "pronouns missing: {body}");
    assert!(
        body.contains("https://pub.example"),
        "profile link missing: {body}"
    );
    assert!(
        body.contains("PM EDT") || body.contains("AM EDT"),
        "local time (EDT) missing: {body}"
    );
    // A messageable target offers the Message link.
    assert!(
        body.contains(&format!("/dm/{}", s.public_id)),
        "message link missing: {body}"
    );
}

#[tokio::test]
async fn private_bio_hidden_from_other_member() {
    let s = setup().await;
    let (status, body) = hovercard(&s.app, Some(&s.member_session), &s.private_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("@privateuser"), "username missing: {body}");
    assert!(
        !body.contains("secret private bio"),
        "private bio leaked to non-admin non-self: {body}"
    );
    // LC-533: the extras share the bio gate, so none leak to a stranger.
    assert!(!body.contains("she/her"), "private pronouns leaked: {body}");
    assert!(
        !body.contains("https://priv.example"),
        "private link leaked: {body}"
    );
    assert!(!body.contains("BST"), "private local time leaked: {body}");
    // Private-profile stranger with no existing DM: no Message link.
    assert!(
        !body.contains(&format!("/dm/{}", s.private_id)),
        "message link shown for non-messageable target: {body}"
    );
}

#[tokio::test]
async fn private_bio_visible_to_admin() {
    let s = setup().await;
    let (status, body) = hovercard(&s.app, Some(&s.admin_session), &s.private_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("secret private bio"),
        "admin should see private bio: {body}"
    );
}

#[tokio::test]
async fn own_card_has_no_message_link() {
    let s = setup().await;
    let (status, body) = hovercard(&s.app, Some(&s.member_session), &s.member_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        !body.contains(&format!("/dm/{}", s.member_id)),
        "self card should not offer Message: {body}"
    );
}

#[tokio::test]
async fn own_card_offers_edit_profile_link() {
    // LC-809: the display-name control is reachable straight from your own
    // name/avatar in the main UI. The self card swaps Message for Edit profile.
    let s = setup().await;
    let (status, body) = hovercard(&s.app, Some(&s.member_session), &s.member_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("/settings#profile"),
        "self card should link to the profile settings: {body}"
    );
}

#[tokio::test]
async fn other_card_has_no_edit_profile_link() {
    // LC-809: only your own card offers the edit-profile shortcut.
    let s = setup().await;
    let (status, body) = hovercard(&s.app, Some(&s.member_session), &s.public_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        !body.contains("/settings#profile"),
        "another user's card must not offer edit-profile: {body}"
    );
}

#[tokio::test]
async fn unknown_user_is_not_found() {
    let s = setup().await;
    let (status, _) = hovercard(&s.app, Some(&s.member_session), "does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unauthenticated_redirects_to_login() {
    let s = setup().await;
    let (status, _) = hovercard(&s.app, None, &s.public_id).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}
