//! LC-776: `/assets/*` claims a one-year immutable cache lifetime for a
//! version-busted URL.
//!
//! Two halves, and both are load-bearing. The route half asserts the header is
//! actually on the wire for `?v=` and deliberately absent otherwise, so an
//! unversioned URL is never pinned at a stale copy for a year. The shape half
//! asserts every runtime `/assets/` URL the app emits carries `?v=`, since the
//! header is only safe while that holds - a new reference added without the
//! query is exactly the regression that makes the caching wrong.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{routes, state::AppState, ws::hub::Hub};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

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

async fn get(app: &Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn cache_control(resp: &axum::response::Response) -> Option<&str> {
    resp.headers()
        .get(header::CACHE_CONTROL)
        .map(|v| v.to_str().expect("ascii cache-control"))
}

#[tokio::test]
async fn versioned_asset_is_immutable_for_a_year() {
    let app = app().await;
    let resp = get(&app, "/assets/main.css?v=testver").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(cache_control(&resp), Some(IMMUTABLE));
}

#[tokio::test]
async fn unversioned_asset_is_not_pinned() {
    // A bare URL (no `?v=`) must NOT get the immutable header, or a rebuilt file
    // would be stranded at a stale copy for a year. The app now version-busts
    // the icons (LC-794), but the withhold-on-bare invariant still holds for any
    // reference that arrives without a version.
    let app = app().await;
    let resp = get(&app, "/assets/icon-192.png").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(cache_control(&resp), None);
}

#[tokio::test]
async fn empty_version_does_not_count_as_versioned() {
    let app = app().await;
    let resp = get(&app, "/assets/main.css?v=").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(cache_control(&resp), None);
}

#[tokio::test]
async fn missing_asset_is_never_cached() {
    // A 404 body pinned for a year would survive the deploy that adds the file.
    let app = app().await;
    let resp = get(&app, "/assets/no-such-file-lc776.css?v=testver").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(cache_control(&resp), None);
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8(bytes.to_vec()).expect("utf-8 body")
}

#[tokio::test]
async fn manifest_is_version_substituted_and_immutable() {
    // LC-794: the manifest is served from a route that substitutes the asset
    // version, so its icon `src` values carry `?v=` and the whole response is
    // immutable-cacheable like every other versioned asset.
    let app = app().await;
    let resp = get(&app, "/assets/manifest.webmanifest?v=testver").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(cache_control(&resp), Some(IMMUTABLE));
    let body = body_string(resp).await;
    assert!(
        !body.contains("__ASSET_VERSION__"),
        "token left unsubstituted"
    );
    for icon in [
        "/assets/favicon.svg?v=testver",
        "/assets/icon-192.png?v=testver",
        "/assets/icon-512.png?v=testver",
        "/assets/icon-maskable-512.png?v=testver",
    ] {
        assert!(
            body.contains(icon),
            "manifest missing versioned icon {icon}"
        );
    }
}

#[tokio::test]
async fn offline_page_is_version_substituted_and_immutable() {
    // LC-794: same treatment for the service worker's navigation fallback, so
    // its favicon `href` carries the version.
    let app = app().await;
    let resp = get(&app, "/assets/offline.html?v=testver").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(cache_control(&resp), Some(IMMUTABLE));
    let body = body_string(resp).await;
    assert!(
        !body.contains("__ASSET_VERSION__"),
        "token left unsubstituted"
    );
    assert!(
        body.contains("/assets/favicon.svg?v=testver"),
        "offline page missing versioned favicon"
    );
}

// ---------------------------------------------------------------------------
// Shape guard: every runtime /assets/ URL carries the version bust.
// ---------------------------------------------------------------------------

/// URLs deliberately left unversioned, keyed by the file that emits them.
///
/// LC-794 closed the icon-family hole: `manifest.webmanifest` and `offline.html`
/// are now served from routes that substitute the asset version, so `base.html`
/// and `sw.js` version-bust the icons too. What remains is the dev theme gallery
/// (no `asset_version` field at all, see its own header comment) and the web-push
/// payload icon (built with no request context; see its row below).
const UNVERSIONED_ALLOWED: &[(&str, &str)] = &[
    ("templates/dev/theme_gallery.html", "/assets/main.css"),
    (
        "templates/dev/theme_gallery.html",
        "/assets/tailwind-built.css",
    ),
    // The web-push payload is built with no request context and rendered by
    // the OS, not the page; it is fetched once per notification, not per
    // navigation.
    ("src/push/payload.rs", "/assets/notification-icon.png"),
];

/// Collect the files whose `/assets/` string literals the app actually
/// requests: the askama templates, the hand-written browser scripts, and the
/// server sources that hand a URL to a client (the push payload's icon).
fn scanned_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("templates"),
        "html",
        &mut out,
    );
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"),
        "js",
        &mut out,
    );
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        "rs",
        &mut out,
    );
    // `vendor/` is third-party and `*.test.js` are node test harnesses; neither
    // emits an app asset URL.
    out.retain(|p| {
        !p.components().any(|c| c.as_os_str() == "vendor")
            && !p.to_string_lossy().ends_with(".test.js")
    });
    out.sort();
    assert!(!out.is_empty(), "scan found no files");
    out
}

fn collect(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// Every `/assets/...` that opens a string literal, i.e. the character before
/// it is a quote. That skips the prose mentions in comments (`// see
/// /assets/outbox.js`) and matches every real URL the app builds. Returns
/// (line, url, is_version_busted).
fn asset_urls(src: &str) -> Vec<(usize, String, bool)> {
    let mut out = Vec::new();
    for (idx, _) in src.match_indices("/assets/") {
        let prev = src[..idx].chars().next_back();
        if !matches!(prev, Some('"') | Some('\'')) {
            continue;
        }
        let rest = &src[idx..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || "._/-".contains(c)))
            .unwrap_or(rest.len());
        // A bare `'/assets/'` is a prefix test (the service worker's fetch
        // router), not a URL.
        if end == "/assets/".len() {
            continue;
        }
        let line = src[..idx].lines().count();
        out.push((
            line,
            rest[..end].to_string(),
            rest[end..].starts_with("?v="),
        ));
    }
    out
}

#[test]
fn every_emitted_asset_url_is_version_busted() {
    let mut violations = Vec::new();
    for path in scanned_files() {
        let rel = path
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(&path).expect("read source");
        for (line, url, versioned) in asset_urls(&src) {
            if versioned || UNVERSIONED_ALLOWED.contains(&(rel.as_str(), url.as_str())) {
                continue;
            }
            violations.push(format!("{rel}:{line}: {url}"));
        }
    }
    assert!(
        violations.is_empty(),
        "these /assets/ URLs carry no ?v= cache bust, so the LC-776 immutable \
         header would strand them at a stale copy. Add `?v=` (asset_version in \
         a template, the data-lc-asset-version attribute in a script), or add \
         the URL to UNVERSIONED_ALLOWED with the reason:\n  {}",
        violations.join("\n  ")
    );
}
