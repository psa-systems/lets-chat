//! LC-777: responses are gzip / brotli compressed when the client asks, and
//! left untouched when it does not. The layer sits outside the branding
//! rewrite, so the decompressed HTML must still carry the brand `<style>`.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{routes, state::AppState, ws::hub::Hub};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// `ServeDir::new("server/assets")` is relative to the process cwd, which is
/// the package dir under `cargo test`. Point it at the workspace root once so
/// the asset requests below resolve to real files.
fn chdir_to_workspace_root() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        std::env::set_current_dir(workspace_root()).expect("chdir to workspace root");
    });
}

async fn app() -> Router {
    chdir_to_workspace_root();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "testver".into(),
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
    routes::build_router(state)
}

async fn get(app: &Router, uri: &str, accept: Option<&str>) -> axum::response::Response {
    let mut req = Request::builder().method(Method::GET).uri(uri);
    if let Some(a) = accept {
        req = req.header(header::ACCEPT_ENCODING, a);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn header_str(resp: &axum::response::Response, name: header::HeaderName) -> Option<&str> {
    resp.headers().get(name).map(|v| v.to_str().unwrap())
}

async fn bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn asset_is_gzipped_and_much_smaller() {
    let app = app().await;
    let raw = bytes(get(&app, "/assets/main.css", None).await).await;
    let resp = get(&app, "/assets/main.css", Some("gzip")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, header::CONTENT_ENCODING), Some("gzip"));
    let vary = header_str(&resp, header::VARY)
        .unwrap()
        .to_ascii_lowercase();
    assert!(vary.contains("accept-encoding"), "vary: {vary}");
    let body = bytes(resp).await;
    assert!(body.len() * 3 < raw.len(), "compressed size {}", body.len());
    let br = bytes(get(&app, "/assets/main.css", Some("br")).await).await;
    assert!(br.len() < 60_000, "brotli size {}", br.len());
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(&body[..])
        .read_to_end(&mut plain)
        .unwrap();
    assert_eq!(plain, raw);
}

#[tokio::test]
async fn brotli_is_negotiated() {
    let app = app().await;
    let resp = get(&app, "/assets/main.css", Some("br")).await;
    assert_eq!(header_str(&resp, header::CONTENT_ENCODING), Some("br"));
}

#[tokio::test]
async fn no_accept_encoding_is_uncompressed() {
    let app = app().await;
    let resp = get(&app, "/assets/main.css", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
    assert!(bytes(resp).await.len() > 200_000);
}

#[tokio::test]
async fn compressed_html_keeps_brand_block_and_security_headers() {
    let app = app().await;
    let resp = get(&app, "/login", Some("gzip")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, header::CONTENT_ENCODING), Some("gzip"));
    assert!(resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).is_some());
    let body = bytes(resp).await;
    let mut html = String::new();
    flate2::read::GzDecoder::new(&body[..])
        .read_to_string(&mut html)
        .unwrap();
    assert!(
        html.contains("<style data-lc-brand>"),
        "brand block missing"
    );
}
