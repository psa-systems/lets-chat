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
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::db;
use crate::error::AppError;
use crate::sso::{oidc, secret};
use crate::state::AppState;

/// 10-minute window between `/auth/sso/:provider/start` and the
/// matching `callback`. Tightening below this risks tripping users on
/// slow IdP login pages (Google's account chooser + 2FA can take
/// minutes); widening past it lets a stolen `state` value linger.
const FLOW_TTL_SECONDS: i64 = 600;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/sso/{provider}/start", get(get_start))
        .route("/auth/sso/{provider}/callback", get(get_callback))
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

#[derive(Deserialize)]
pub struct CallbackQuery {
    /// Authorization code returned by the IdP. Empty when the user
    /// declined / errored out.
    pub code: Option<String>,
    /// CSRF state token; must match the `flow_id` we wrote in `start`.
    pub state: Option<String>,
    /// OIDC error code (e.g. `access_denied`). When present, the
    /// callback short-circuits without attempting token exchange.
    pub error: Option<String>,
    pub error_description: Option<String>,
}

pub async fn get_callback(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Query(q): Query<CallbackQuery>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if let Some(err) = q.error.as_deref() {
        let desc = q.error_description.as_deref().unwrap_or("");
        tracing::warn!(provider = %provider_id, error = %err, description = %desc, "SSO callback returned error");
        return Err(AppError::BadRequest(format!(
            "sign-in failed at identity provider: {err}"
        )));
    }
    let state_token = q
        .state
        .ok_or_else(|| AppError::BadRequest("missing `state` parameter".into()))?;
    let code = q
        .code
        .ok_or_else(|| AppError::BadRequest("missing `code` parameter".into()))?;

    let flow = db::sso::consume_sso_flow(&state.auth, &state_token)
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(
                "this sign-in link has expired or was already used; please start over".into(),
            )
        })?;
    if flow.provider_id != provider_id {
        // The state was minted for a different provider. Refuse rather
        // than silently honouring it - a path/state mismatch is either
        // a copy-paste bug or an attempt to splice flows.
        return Err(AppError::BadRequest("provider mismatch on callback".into()));
    }
    if flow.kind != "sign_in" {
        return Err(AppError::BadRequest(format!(
            "unexpected flow kind `{}` on /callback",
            flow.kind
        )));
    }

    let entry = state
        .sso
        .lookup(&provider_id)
        .await
        .ok_or(AppError::NotFound)?;
    let metadata = entry
        .discovery(http_client())
        .await
        .map_err(|e| AppError::Internal(format!("discovery: {e}")))?;
    let key = state
        .secret_key
        .as_ref()
        .ok_or_else(|| AppError::Internal("LETS_CHAT_SECRET_KEY not set".into()))?;
    let client_secret =
        secret::decrypt_client_secret(key.as_ref(), &entry.row.client_secret_encrypted)
            .map_err(|e| AppError::Internal(format!("decrypt client_secret: {e}")))?;

    let redirect_uri = format!(
        "{}/auth/sso/{}/callback",
        state.base_url.trim_end_matches('/'),
        provider_id
    );
    let token = oidc::exchange_code(
        http_client(),
        &metadata.token_endpoint,
        &code,
        &flow.pkce_verifier,
        &entry.row.client_id,
        &client_secret,
        &redirect_uri,
    )
    .await
    .map_err(|e| {
        tracing::warn!(provider = %provider_id, error = %e, "token exchange failed");
        AppError::BadRequest("sign-in failed; check with your admin.".into())
    })?;
    let id_token = token.id_token.ok_or_else(|| {
        AppError::BadRequest("identity provider did not return an id_token".into())
    })?;

    let claims = oidc::verify_id_token(
        &id_token,
        &metadata.jwks_json,
        &metadata.issuer,
        &entry.row.client_id,
        &flow.nonce,
    )
    .map_err(|e| {
        tracing::warn!(provider = %provider_id, error = %e, "id_token verification failed");
        AppError::BadRequest("sign-in failed; identity could not be verified.".into())
    })?;

    // Already-linked happy path.
    if let Some(user_id) =
        db::sso::find_user_by_sso(&state.auth, &metadata.issuer, &claims.sub).await?
    {
        // Touch the link's `last_seen_at` + refresh the stored email
        // metadata if it changed. The `auto_linked` flag stays as-is
        // (sticky via the UPSERT).
        db::sso::link_sso_identity(
            &state.auth,
            &user_id,
            &metadata.issuer,
            &claims.sub,
            claims.email.as_deref(),
            false,
        )
        .await?;
        return finalize_sign_in(
            &state,
            &user_id,
            &provider_id,
            &flow.return_to,
            &headers,
            jar,
        )
        .await;
    }

    // Auto-link on email match. Gated on the per-provider flag AND
    // the IdP's `email_verified=true` claim. Per doc 02 section 2 / 10 section 1.
    if entry.row.auto_link_verified_email
        && claims.email_verified == Some(true)
        && claims.email.is_some()
    {
        let email = claims.email.as_deref().unwrap();
        if let Some(user_id) = db::auth::find_user_id_by_email(&state.auth, email).await? {
            db::sso::link_sso_identity(
                &state.auth,
                &user_id,
                &metadata.issuer,
                &claims.sub,
                Some(email),
                true,
            )
            .await?;
            tracing::warn!(
                target: "lets_chat.auth.sso",
                event = "sso_account_auto_linked",
                user_id = %user_id,
                provider = %provider_id,
                issuer = %metadata.issuer,
                subject = %claims.sub,
                email = %email,
                "auto-linked existing account on verified email match"
            );
            return finalize_sign_in(
                &state,
                &user_id,
                &provider_id,
                &flow.return_to,
                &headers,
                jar,
            )
            .await;
        }
    }

    // Email collision but auto-link is off OR email_verified is false:
    // fall through to the link-required interstitial. That page lives
    // in L13; for now, return a placeholder explanation.
    if let Some(email) = claims.email.as_deref() {
        if db::auth::find_user_id_by_email(&state.auth, email)
            .await?
            .is_some()
        {
            tracing::info!(
                target: "lets_chat.auth.sso",
                provider = %provider_id,
                email = %email,
                "sso callback: existing account by email; link-required interstitial (L13) not yet implemented"
            );
            return Err(AppError::BadRequest(
                "An account already exists for this email. \
                 Sign in with your password first to link it. \
                 (link-required interstitial lands in L13.)"
                    .into(),
            ));
        }
    }

    // No link, no email match. Auto-provision (L14) branches off here.
    tracing::info!(
        target: "lets_chat.auth.sso",
        provider = %provider_id,
        subject = %claims.sub,
        email = ?claims.email,
        "sso callback: unknown identity (L14 auto-provision not yet implemented)"
    );
    Err(AppError::BadRequest(
        "Your account isn't authorized for this deployment. \
         Ask an admin to invite you. \
         (auto-provisioning lands in L14.)"
            .into(),
    ))
}

/// Mint the session cookie + emit the `sso_sign_in` tracing event +
/// 302 to `return_to`. Shared between the already-linked and
/// auto-linked branches above.
async fn finalize_sign_in(
    state: &AppState,
    user_id: &str,
    provider_id: &str,
    return_to: &str,
    headers: &HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let (ua, ip) = crate::auth::extract_session_origin(headers);
    let session_token =
        db::auth::create_session_with_origin(&state.auth, user_id, ua.as_deref(), ip.as_deref())
            .await?;
    let cookie = crate::routes::auth::build_session_cookie(state.cookies_secure(), session_token);
    let jar = jar.add(cookie);
    tracing::info!(
        target: "lets_chat.auth.sso",
        user_id = %user_id,
        provider = %provider_id,
        "sso_sign_in"
    );
    Ok((jar, Redirect::to(return_to)).into_response())
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
