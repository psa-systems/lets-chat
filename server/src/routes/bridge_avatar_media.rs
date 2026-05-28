//! LC-78-AVATAR-PROXY: GET `/media/bridge-avatar-proxy/{hash}` serves a
//! cached foreign-avatar blob from disk. AuthUser-gated. Threat-model
//! framing per the design plan's sharpening #2:
//!
//! > Auth-gating here does NOT enforce room-of-origin scoping. The hash is
//! > opaque but enumerable from any cached HTML that references it; the
//! > gate is to prevent **anonymous** fetches of leaked hashes (referer
//! > logs, screenshots, misconfigured shares) and to leave an authed
//! > access trail in request logs. Room-of-origin scoping comes from the
//! > rendering surface controlling who sees the hash in the first place,
//! > not from this gate.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// Hard cap on hash length: sha256 hex is 64 chars. Reject anything else
/// to bound path-traversal exposure even though `Path` strips slashes.
const MAX_HASH_LEN: usize = 64;

pub async fn get_bridge_avatar(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(hash): Path<String>,
) -> Result<Response, AppError> {
    if !is_valid_hash(&hash) {
        return Err(AppError::NotFound);
    }
    if !crate::bridge_avatar::proxy_enabled() {
        // Gate-off mode: 404 every hash. Same shape as a non-existent row,
        // so no fingerprinting between "feature off" and "hash unknown".
        return Err(AppError::NotFound);
    }
    let Some(row) = db::bridge_avatar_proxies::find_by_hash(&state.chat, &hash).await? else {
        return Err(AppError::NotFound);
    };
    if row.fetch_status != "ok" {
        // pending: fetch task hasn't completed (or it crashed; sweep_pending_orphans
        // will mark it failed eventually).
        // failed: terminal in v2; render falls back to initials via <img onerror>.
        return Err(AppError::NotFound);
    }
    let path = db::bridge_avatars_dir().join(&hash);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Row says ok but file is missing (manual deletion, sweep race).
            // 404 to the viewer; an admin can rebuild by clearing the row
            // and re-triggering on the next bridge message that references
            // the foreign URL.
            return Err(AppError::NotFound);
        }
        Err(e) => return Err(AppError::Internal(format!("read avatar: {e}"))),
    };
    let content_type = row
        .content_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            // Content-addressed by sha256: safe to long-cache. Bytes for
            // a given hash never change; if a foreign URL's content changes
            // upstream, the daemon submits a new message with a new URL ->
            // new hash -> new cache row.
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_string(),
            ),
        ],
        bytes,
    )
        .into_response())
}

fn is_valid_hash(s: &str) -> bool {
    s.len() == MAX_HASH_LEN
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Test seam: confirm hash validation is strict. Belt-and-braces on top of
/// Axum's `Path` extractor (which strips slashes), but ensures any future
/// router change that swaps in a permissive matcher does not let a
/// malformed path through.
#[cfg(test)]
mod tests {
    use super::is_valid_hash;

    #[test]
    fn accepts_64_char_lowercase_hex() {
        assert!(is_valid_hash(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
    }

    #[test]
    fn rejects_uppercase() {
        assert!(!is_valid_hash(
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        ));
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(!is_valid_hash("abcd"));
        assert!(!is_valid_hash(&"a".repeat(63)));
        assert!(!is_valid_hash(&"a".repeat(65)));
    }

    #[test]
    fn rejects_non_hex() {
        assert!(!is_valid_hash(
            "../../etc/passwd-padded-out-to-sixty-four-bytes-for-this-test!!!"
        ));
        assert!(!is_valid_hash(
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
        ));
    }
}
