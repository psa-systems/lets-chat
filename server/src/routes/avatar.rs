use std::hash::{Hash, Hasher};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// LC-781 (F11): a `?v=<token>` marks a versioned avatar URL. Its presence (not
/// its value) flips the response to `immutable`; the token only has to differ
/// across uploads, which `crate::avatar_version` guarantees.
#[derive(Deserialize)]
pub struct AvatarQuery {
    #[serde(default)]
    v: Option<String>,
}

/// Cache-Control for the avatar response. A versioned request (`?v=...`) is safe
/// to cache forever - the URL changes when the image does - so it skips
/// revalidation entirely. A bare request keeps LC-348's `no-cache` + ETag, so a
/// not-yet-versioned URL still reflects an upload on the next request.
fn cache_control(versioned: bool) -> &'static str {
    if versioned {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

pub async fn get_avatar(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    Query(query): Query<AvatarQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let versioned = query.v.is_some();
    // LC-701: an unknown id (a message author who is a bot or a since-deleted
    // user, e.g. surfaced in search results) must still resolve to an image, not
    // 404. `/avatars/{id}` is referenced unconditionally by chat rows, the voice
    // grid, and the search result rows, none of which can branch on existence;
    // a 404 there is a broken <img> and console noise. Fall back to a generated
    // default so the invariant "this route always resolves to an image" holds.
    let Some(user) = db::auth::find_user_by_id(&state.auth, &user_id).await? else {
        // Empty username -> the generic "?" initial (default_avatar's fallback).
        return Ok(default_avatar("", &headers));
    };
    let Some(ext) = user.avatar_ext else {
        // No custom avatar: serve a generated initial-on-color SVG so
        // `/avatars/{id}` always resolves to an image (used by the voice
        // grid and DM call overlay, which can't branch on avatar presence).
        return Ok(default_avatar(&user.username, &headers));
    };
    let path = db::avatars_dir().join(format!("{user_id}.{ext}"));
    // LC-348: stat first so the ETag reflects the current file. The bare
    // `/avatars/{id}` URL is shared by chat rows, the sidebar, hovercards, and
    // the voice grid; without revalidation a `max-age` cache showed the old
    // avatar (or the previously-cached default initial) for the whole TTL after
    // an upload. ETag + `no-cache` makes a changed avatar appear on the next
    // request while unchanged ones cost only a 304.
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let etag = file_etag(&ext, &meta);
    if if_none_match(&headers, &etag) {
        return Ok(not_modified(&etag, versioned));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CACHE_CONTROL, cache_control(versioned).to_string()),
            (header::ETAG, etag),
        ],
        bytes,
    )
        .into_response())
}

/// Generated fallback avatar: the user's first initial on a flat slate
/// background, mirroring the CSS default in `partials/avatar.html`. Returned
/// as an SVG so it scales cleanly in every avatar slot.
fn default_avatar(username: &str, headers: &HeaderMap) -> Response {
    let initial = username
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"128\" height=\"128\" \
         viewBox=\"0 0 128 128\"><rect width=\"128\" height=\"128\" fill=\"#cbd5e1\"/>\
         <text x=\"64\" y=\"64\" font-family=\"sans-serif\" font-size=\"60\" font-weight=\"600\" \
         fill=\"#334155\" text-anchor=\"middle\" dominant-baseline=\"central\">{initial}</text></svg>"
    );
    // LC-348: content-based ETag, prefixed `d` so it can never collide with a
    // real file ETag. The moment the user uploads an avatar this route switches
    // to the file branch, whose ETag differs, so the browser refetches.
    let etag = format!("\"d{:x}\"", hash_str(&svg));
    if if_none_match(headers, &etag) {
        return not_modified(&etag, false);
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml".to_string()),
            // Default SVG is username-derived, not file-versioned: always no-cache
            // so a username change reflects immediately (LC-781 F11 keeps LC-348).
            (header::CACHE_CONTROL, cache_control(false).to_string()),
            (header::ETAG, etag),
        ],
        svg,
    )
        .into_response()
}

/// Strong-ish ETag for a stored avatar file: extension + byte length + mtime
/// nanos. Any re-upload changes the mtime (and usually the length), so the tag
/// moves whenever the image does.
fn file_etag(ext: &str, meta: &std::fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("\"{ext}-{}-{mtime}\"", meta.len())
}

/// True when one of the client's `If-None-Match` tags equals `etag`.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
}

fn not_modified(etag: &str, versioned: bool) -> Response {
    (
        StatusCode::NOT_MODIFIED,
        [
            (header::CACHE_CONTROL, cache_control(versioned).to_string()),
            (header::ETAG, etag.to_string()),
        ],
    )
        .into_response()
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
