//! LC-348: the `/avatars/{id}` route must let a changed avatar appear promptly
//! on every surface that shares the bare URL (chat rows, sidebar, hovercards,
//! voice grid). It serves `Cache-Control: no-cache` + an `ETag` and answers a
//! matching `If-None-Match` with `304`, so the browser revalidates instead of
//! holding a stale `max-age` copy for the whole TTL.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<()> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

struct Setup {
    app: Router,
    auth: sqlx::SqlitePool,
    user_id: String,
    session: String,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let user_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
    };
    Setup {
        app: routes::build_router(state),
        auth,
        user_id,
        session,
    }
}

async fn get_avatar(
    app: &Router,
    sess: &str,
    user_id: &str,
    if_none_match: Option<&str>,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(format!("/avatars/{user_id}"))
        .header(header::COOKIE, format!("session={sess}"));
    if let Some(etag) = if_none_match {
        req = req.header(header::IF_NONE_MATCH, etag);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn header_str(resp: &axum::response::Response, name: header::HeaderName) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn default_avatar_revalidates_with_etag() {
    let s = setup().await;
    // No avatar_ext set: served as the generated SVG, but still revalidating.
    let resp = get_avatar(&s.app, &s.session, &s.user_id, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header_str(&resp, header::CONTENT_TYPE),
        "image/svg+xml",
        "default avatar should be the generated SVG"
    );
    assert_eq!(header_str(&resp, header::CACHE_CONTROL), "no-cache");
    let etag = header_str(&resp, header::ETAG);
    assert!(
        etag.starts_with("\"d"),
        "default etag must be content-based and `d`-prefixed, got {etag}"
    );

    // A matching If-None-Match short-circuits to 304.
    let resp = get_avatar(&s.app, &s.session, &s.user_id, Some(&etag)).await;
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn uploaded_avatar_serves_etag_then_304() {
    let s = setup().await;

    // Simulate an uploaded avatar: write the file and set the ext (what
    // post_profile does after sniffing the bytes).
    let dir = db::avatars_dir();
    tokio::fs::write(
        dir.join(format!("{}.jpg", s.user_id)),
        b"\xff\xd8\xffjpegbytes",
    )
    .await
    .unwrap();
    db::auth::set_user_avatar_ext(&s.auth, &s.user_id, Some("jpg"))
        .await
        .unwrap();

    let resp = get_avatar(&s.app, &s.session, &s.user_id, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, header::CONTENT_TYPE), "image/jpeg");
    assert_eq!(header_str(&resp, header::CACHE_CONTROL), "no-cache");
    let etag = header_str(&resp, header::ETAG);
    assert!(!etag.is_empty(), "uploaded avatar must carry an ETag");
    assert!(
        !etag.starts_with("\"d"),
        "file etag must not look like a default-avatar etag"
    );

    // Revalidation with the served ETag yields 304 (browser keeps its copy).
    let resp = get_avatar(&s.app, &s.session, &s.user_id, Some(&etag)).await;
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);

    // A stale/old ETag does NOT 304: the changed avatar is delivered fresh.
    let resp = get_avatar(&s.app, &s.session, &s.user_id, Some("\"jpg-1-1\"")).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let _ = tokio::fs::remove_file(dir.join(format!("{}.jpg", s.user_id))).await;
}
