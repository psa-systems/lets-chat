//! Public SSO routes: `/auth/sso/:provider/start` and (in L11)
//! `/auth/sso/:provider/callback`.
//!
//! L9 ships only the `start` half of the OIDC code+PKCE dance:
//!   1. Resolve `:provider` -> cached `ProviderEntry`. 404 if not enabled.
//!   2. Fetch / reuse cached discovery metadata.
//!   3. Generate PKCE verifier + S256 challenge, random `state`, random `nonce`.
//!   4. Write the `sso_flows` row (10-minute TTL).
//!   5. 302 to the IdP's `authorization_endpoint` with the OIDC query params.
//!
//! The callback in L11 picks up by looking the `state` value up in
//! `sso_flows`, verifying the returned id_token, and minting a session.

use std::sync::OnceLock;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// 10-minute window between `/auth/sso/:provider/start` and the
/// matching `callback`. Tightening below this risks tripping users on
/// slow IdP login pages (Google's account chooser + 2FA can take
/// minutes); widening past it lets a stolen `state` value linger.
const FLOW_TTL_SECONDS: i64 = 600;

pub fn router() -> Router<AppState> {
    Router::new().route("/auth/sso/{provider}/start", get(get_start))
}

#[derive(Deserialize)]
pub struct StartQuery {
    /// Where the user came from. Validated server-side against an
    /// allow-list of internal relative paths so a `return_to` query
    /// param can't smuggle an open redirect through the IdP round-trip.
    pub return_to: Option<String>,
}

pub async fn get_start(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Query(q): Query<StartQuery>,
) -> Result<Response, AppError> {
    let entry = state
        .sso
        .lookup(&provider_id)
        .await
        .ok_or(AppError::NotFound)?;
    let metadata = entry.discovery(http_client()).await.map_err(|e| {
        tracing::warn!(provider = %provider_id, error = %e, "OIDC discovery failed");
        AppError::Internal(format!("discovery: {e}"))
    })?;

    let return_to = sanitize_return_to(q.return_to.as_deref());
    let state_token = random_b64url(32);
    let nonce = random_b64url(32);
    let pkce_verifier = random_b64url(48);
    let pkce_challenge = s256_challenge(&pkce_verifier);

    db::sso::insert_sso_flow(
        &state.auth,
        &state_token,
        &state_token,
        &nonce,
        &pkce_verifier,
        &return_to,
        "sign_in",
        None,
        &provider_id,
        FLOW_TTL_SECONDS,
    )
    .await?;

    let redirect_uri = format!(
        "{}/auth/sso/{}/callback",
        state.base_url.trim_end_matches('/'),
        provider_id
    );
    let mut auth_url = metadata.authorization_endpoint.clone();
    auth_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &entry.row.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &entry.row.scopes)
        .append_pair("state", &state_token)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &pkce_challenge)
        .append_pair("code_challenge_method", "S256");

    Ok(Redirect::to(auth_url.as_str()).into_response())
}

/// Validate the `return_to` query parameter as a safe internal path.
/// Falls back to `/` when the input is missing or unsafe.
///
/// Rules:
///   - must start with `/`
///   - must NOT start with `//` (would be a protocol-relative URL)
///   - must NOT start with `/\` (Edge / IE treat backslashes as `/`)
///   - must NOT contain a scheme (`http:`, `javascript:`, etc.) anywhere
///     before the first slash - already covered by the leading-slash
///     check, but we also reject control characters out of caution.
pub fn sanitize_return_to(raw: Option<&str>) -> String {
    let candidate = raw.unwrap_or("/").trim();
    if !candidate.starts_with('/') {
        return "/".to_string();
    }
    if candidate.starts_with("//") || candidate.starts_with("/\\") {
        return "/".to_string();
    }
    if candidate.chars().any(|c| c.is_control()) {
        return "/".to_string();
    }
    candidate.to_string()
}

fn random_b64url(bytes_len: usize) -> String {
    let mut buf = vec![0u8; bytes_len];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(&buf)
}

fn s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Process-wide shared `reqwest::Client` for discovery. Reqwest keeps
/// a connection pool inside the client; reusing one avoids spinning a
/// fresh pool per request. Eagerly built on first call.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_returns_root_for_empty() {
        assert_eq!(sanitize_return_to(None), "/");
        assert_eq!(sanitize_return_to(Some("")), "/");
        assert_eq!(sanitize_return_to(Some("   ")), "/");
    }

    #[test]
    fn sanitize_rejects_absolute_urls() {
        assert_eq!(sanitize_return_to(Some("https://evil.example/")), "/");
        assert_eq!(sanitize_return_to(Some("http://evil")), "/");
        assert_eq!(sanitize_return_to(Some("//evil.example/")), "/");
        assert_eq!(sanitize_return_to(Some("javascript:alert(1)")), "/");
    }

    #[test]
    fn sanitize_rejects_backslash_smuggling() {
        assert_eq!(sanitize_return_to(Some("/\\evil")), "/");
    }

    #[test]
    fn sanitize_rejects_control_chars() {
        assert_eq!(sanitize_return_to(Some("/path\r\nLocation: x")), "/");
    }

    #[test]
    fn sanitize_keeps_internal_paths() {
        assert_eq!(sanitize_return_to(Some("/rooms/general")), "/rooms/general");
        assert_eq!(
            sanitize_return_to(Some("/settings/profile?x=1")),
            "/settings/profile?x=1"
        );
    }

    #[test]
    fn s256_matches_rfc7636_test_vector() {
        // RFC 7636 appendix B vectors.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            s256_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_b64url_is_url_safe_and_full_length() {
        let s = random_b64url(32);
        assert!(!s.contains('+'));
        assert!(!s.contains('/'));
        assert!(!s.contains('='));
        // 32 bytes -> 43 chars unpadded.
        assert_eq!(s.len(), 43);
    }
}
