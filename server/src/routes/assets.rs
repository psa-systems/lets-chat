//! LC-794: serve `manifest.webmanifest` and `offline.html` from routes that
//! substitute the build's asset version, mirroring `push::get_service_worker`
//! for `sw.js`.
//!
//! Both are otherwise static files served straight off disk by `ServeDir`, so
//! their icon / favicon references cannot interpolate `asset_version` and had to
//! be requested bare. That left the icon family on heuristic revalidation - the
//! exact cost LC-776's immutable cache removed everywhere else. Serving them
//! from a handler that replaces `__ASSET_VERSION__` lets every reference carry
//! `?v=`, so the immutable header covers the whole `/assets` directory.
//!
//! Neither handler sets `Cache-Control`; the `cache_static_assets` layer adds
//! the immutable header when the request carries `?v=`, exactly as it does for
//! every other versioned asset.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// GET /assets/manifest.webmanifest - the PWA manifest with its four icon
/// `src` values version-busted.
pub async fn get_manifest(State(state): State<AppState>) -> Response {
    let body = include_str!("../../assets/manifest.webmanifest")
        .replace("__ASSET_VERSION__", &state.asset_version);
    ([(header::CONTENT_TYPE, "application/manifest+json")], body).into_response()
}

/// GET /assets/offline.html - the service worker's navigation fallback, with
/// its favicon `href` version-busted.
pub async fn get_offline(State(state): State<AppState>) -> Response {
    let body = include_str!("../../assets/offline.html")
        .replace("__ASSET_VERSION__", &state.asset_version);
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}
