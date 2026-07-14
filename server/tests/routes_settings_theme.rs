//! LC-292: the sidebar theme quick-toggle persists via POST /settings/theme.
//! The endpoint saves only `users.theme_mode`, returns 204, and stamps an
//! `lc-mode` cookie so the next navigation has no flash-of-old-theme. An
//! invalid value falls back to "system" instead of 500ing, and the route is
//! auth-gated. LC-541: this file also covers POST /settings/palette.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn post_theme(app: &Router, sess: Option<&str>, theme: &str) -> axum::response::Response {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/settings/theme")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        req = req.header(header::COOKIE, format!("session={s}"));
    }
    let req = req.body(Body::from(format!("theme={theme}"))).unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn post_palette(app: &Router, sess: Option<&str>, palette: &str) -> axum::response::Response {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/settings/palette")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        req = req.header(header::COOKIE, format!("session={s}"));
    }
    let req = req.body(Body::from(format!("palette={palette}"))).unwrap();
    app.clone().oneshot(req).await.unwrap()
}

struct Setup {
    app: Router,
    auth: sqlx::SqlitePool,
    user_id: String,
    session: String,
}

async fn setup() -> Setup {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let user_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    // enforce_2fa_enrollment is active (secret_key is Some); without this every
    // authed request 303s to /settings/2fa/setup.
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        auth: auth.clone(),
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
        auth,
        user_id,
        session,
    }
}

#[tokio::test]
async fn valid_theme_persists_and_sets_cookie() {
    let s = setup().await;
    let resp = post_theme(&s.app, Some(&s.session), "dark").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cookie.contains("lc-mode=dark"), "cookie not set: {cookie}");

    let user = db::auth::find_user_by_id(&s.auth, &s.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.theme_mode.as_deref(), Some("dark"));
}

#[tokio::test]
async fn high_contrast_theme_is_accepted() {
    let s = setup().await;
    let resp = post_theme(&s.app, Some(&s.session), "hc-dark").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let user = db::auth::find_user_by_id(&s.auth, &s.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.theme_mode.as_deref(), Some("hc-dark"));
}

#[tokio::test]
async fn invalid_theme_falls_back_to_system_not_500() {
    let s = setup().await;
    let resp = post_theme(&s.app, Some(&s.session), "bogus").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let user = db::auth::find_user_by_id(&s.auth, &s.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.theme_mode.as_deref(), Some("system"));
}

#[tokio::test]
async fn unauthenticated_post_redirects_to_login() {
    let s = setup().await;
    let resp = post_theme(&s.app, None, "dark").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");
}

#[tokio::test]
async fn valid_palette_persists_and_sets_cookie() {
    let s = setup().await;
    let resp = post_palette(&s.app, Some(&s.session), "cobalt").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        cookie.contains("lc-palette=cobalt"),
        "cookie not set: {cookie}"
    );

    let user = db::auth::find_user_by_id(&s.auth, &s.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.theme_palette.as_deref(), Some("cobalt"));
}

#[tokio::test]
async fn invalid_palette_falls_back_to_blue_harbor_not_500() {
    let s = setup().await;
    let resp = post_palette(&s.app, Some(&s.session), "bogus").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let user = db::auth::find_user_by_id(&s.auth, &s.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.theme_palette.as_deref(), Some("blue-harbor"));
}
