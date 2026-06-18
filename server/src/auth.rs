use std::sync::Arc;
use std::time::{Duration, Instant};

use askama::Template;
use axum::extract::{FromRequestParts, State};
use axum::http::{request::Parts, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;
use dashmap::DashMap;

use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::maintenance::MaintenancePage;

/// Per-session debounce for `sessions.last_seen_at` writes. We bump the
/// column on every authed request, but only flush to SQLite once per
/// session per minute so the write rate stays well below the read rate.
/// The ledger is in-memory; after a restart we resume at the actual
/// `last_seen_at` already in the DB row.
const LAST_SEEN_DEBOUNCE: Duration = Duration::from_secs(60);

pub type LastSeenLedger = Arc<DashMap<String, Instant>>;

pub fn new_last_seen_ledger() -> LastSeenLedger {
    Arc::new(DashMap::new())
}

pub const SESSION_COOKIE: &str = "session";

/// Extract a short identity string for a session row from request headers.
/// Returns `(user_agent, client_ip)`. Both are truncated to keep DB rows
/// small and to defend against pathological headers from clients we don't
/// control. Client IP prefers `X-Forwarded-For` (first hop) and falls back
/// to `X-Real-IP`; when the deployment runs without a reverse proxy these
/// will both be missing and the column stays NULL, which is fine.
/// Extract the User-Agent and client IP for the session audit trail / login
/// alert. LC-155: the IP is read from `X-Forwarded-For` / `X-Real-IP`, both
/// trivially spoofable, so it is only trusted when `trust_proxy` is set (the
/// operator runs behind a reverse proxy that sets them). Without that, the IP
/// is `None` rather than a forgeable value, matching the rate limiter's
/// posture. The User-Agent is always captured: it is display-only, not a
/// security decision.
pub fn extract_session_origin(
    headers: &axum::http::HeaderMap,
    trust_proxy: bool,
) -> (Option<String>, Option<String>) {
    fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    let ua = header_str(headers, "user-agent").map(|s| truncate(&s, 256));
    let ip = if trust_proxy {
        header_str(headers, "x-forwarded-for")
            .and_then(|s| s.split(',').next().map(|p| p.trim().to_string()))
            .filter(|s| !s.is_empty())
            .or_else(|| header_str(headers, "x-real-ip"))
            .map(|s| truncate(&s, 64))
    } else {
        None
    };
    (ua, ip)
}

/// Whether the deployment trusts client-IP proxy headers (`X-Forwarded-For` /
/// `X-Real-IP`). Operator opt-in via the `trust_proxy_headers` setting; the
/// default migration seeds it `true`. Shared by the session-origin extractor
/// and the per-IP rate limiter so both honor one switch.
pub async fn proxy_headers_trusted(settings: &sqlx::SqlitePool) -> bool {
    crate::db::settings::get_setting(settings, "trust_proxy_headers")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("true")
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

/// Wrapper newtype for the current session id, injected alongside `User` so
/// settings handlers can mark the row that issued the request.
#[derive(Clone, Debug)]
pub struct CurrentSessionId(pub String);

/// Middleware: read the session cookie, look up the user, and inject the
/// resulting `User` into request extensions when the session is valid and the
/// account is not banned. Also bumps `sessions.last_seen_at` at most once
/// per `LAST_SEEN_DEBOUNCE` per session.
pub async fn inject_user(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());
    let user = match token.as_deref() {
        Some(t) => match db::auth::get_user_by_session(&state.auth, t).await {
            Ok(Some(record)) if !record.is_banned => Some(User::from(record)),
            _ => None,
        },
        None => None,
    };
    if let (Some(u), Some(t)) = (user, token) {
        req.extensions_mut().insert(u);
        if should_touch_last_seen(&state.last_seen_ledger, &t) {
            state.bg.touch_session(&t);
        }
        req.extensions_mut().insert(CurrentSessionId(t));
    }
    next.run(req).await
}

/// LC-100: resolve the request's UI locale and run the rest of the request
/// inside the `CURRENT_LOCALE` task-local so template `| t` filters localize.
/// Must be layered INSIDE `inject_user` so the signed-in `User` (carrying the
/// saved `locale` preference) is already in the request extensions.
pub async fn resolve_locale(req: axum::extract::Request, next: Next) -> Response {
    use axum::http::header::{ACCEPT_LANGUAGE, SET_COOKIE};
    let user = req.extensions().get::<User>();
    let user_locale = user.and_then(|u| u.locale.clone());
    // LC-191: keep the `lc-theme` cookie (which the no-flash bootstrap reads
    // before paint) in sync with the saved `users.theme`. This is what carries
    // the preference cross-device: a fresh device has no cookie, so the first
    // authed response stamps it from the server pref.
    let user_theme = user.and_then(|u| u.theme.clone());
    // LC-194: density rides the same cross-device mechanism as theme.
    let user_density = user.and_then(|u| u.density.clone());
    let existing_theme = read_cookie(req.headers(), "lc-theme");
    let existing_density = read_cookie(req.headers(), "lc-density");
    let accept = req
        .headers()
        .get(ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let lang = crate::i18n::resolve(user_locale.as_deref(), accept.as_deref());
    let mut resp = crate::i18n::CURRENT_LOCALE.scope(lang, next.run(req)).await;

    // Stamp `name=value` (1-year, Lax) when a valid server pref drifts from the
    // request cookie. Non-HttpOnly so the no-flash bootstrap can read it; the
    // values are non-sensitive UI prefs.
    let mut sync =
        |name: &str, pref: &Option<String>, existing: &Option<String>, valid: &[&str]| {
            if let Some(v) = pref {
                if valid.contains(&v.as_str()) && existing.as_deref() != Some(v.as_str()) {
                    if let Ok(h) = axum::http::HeaderValue::from_str(&format!(
                        "{name}={v}; Path=/; Max-Age=31536000; SameSite=Lax"
                    )) {
                        resp.headers_mut().append(SET_COOKIE, h);
                    }
                }
            }
        };
    sync(
        "lc-theme",
        &user_theme,
        &existing_theme,
        &["light", "dark", "hc-light", "hc-dark", "system"],
    );
    sync(
        "lc-density",
        &user_density,
        &existing_density,
        &["comfortable", "compact"],
    );
    resp
}

/// Read a cookie value by name from a request header map.
fn read_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .map(str::trim)
                .find_map(|p| p.strip_prefix(&prefix))
        })
        .map(str::to_owned)
}

fn should_touch_last_seen(ledger: &LastSeenLedger, session_id: &str) -> bool {
    let now = Instant::now();
    // CRITICAL: drop the Ref from `get` BEFORE calling `insert`. DashMap's
    // shard locks are reentrant only within the same scope; holding a
    // read Ref while requesting a write lock on the same shard
    // deadlocks. The previous match-arm version kept the Some-arm Ref
    // alive through the `_` arm, which is exactly that pattern and
    // caused every authed request to hang forever in inject_user.
    let recent = ledger
        .get(session_id)
        .map(|prev| now.duration_since(*prev) < LAST_SEEN_DEBOUNCE)
        .unwrap_or(false);
    if recent {
        return false;
    }
    ledger.insert(session_id.to_string(), now);
    true
}

/// Extractor: pulls the authenticated `User` from extensions, or 303s to /login.
pub struct AuthUser(pub User);

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(u) = parts.extensions.get::<User>().cloned() {
            Ok(AuthUser(u))
        } else {
            Err(Redirect::to("/login").into_response())
        }
    }
}

/// LC-22 cutover: `enforce_2fa_enrollment` was deleted. Bunyip owns 2FA;
/// every Bunyip-provisioned user has `totp_enabled = 0` and the old
/// middleware would 303 them to a route that no longer exists. If an
/// operator wants stricter 2FA, they configure it on the Bunyip user.
///
/// Middleware: when the `maintenance_mode` setting is on, return a 503
/// maintenance page for everyone except admins. Admins still need the
/// admin area, the SSO surface, and static assets to recover, so each
/// of those is exempt before the setting is even read.
pub async fn enforce_maintenance_mode(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    let exempt = path.starts_with("/assets/")
        || path.starts_with("/avatars/")
        || path == "/login"
        || path == "/logout"
        || path.starts_with("/auth/bunyip/")
        || path == "/version";
    if exempt {
        return next.run(req).await;
    }
    if let Some(u) = req.extensions().get::<User>() {
        if u.role == "admin" {
            return next.run(req).await;
        }
    }
    let enabled = db::settings::get_setting(&state.settings, "maintenance_mode")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("true");
    if !enabled {
        return next.run(req).await;
    }
    let message = db::settings::get_setting(&state.settings, "maintenance_message")
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let body = MaintenancePage {
        message: &message,
        asset_version: &state.asset_version,
    }
    .render()
    .unwrap_or_else(|_| "Maintenance in progress.".to_string());
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Extractor for routes that may render either a public or authed page.
pub struct OptionalUser(pub Option<User>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalUser {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalUser(parts.extensions.get::<User>().cloned()))
    }
}

/// Extractor that requires admin role.
pub struct AdminUser(pub User);

impl<S: Send + Sync> FromRequestParts<S> for AdminUser {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<User>().cloned() {
            Some(u) => match u.role.as_str() {
                "admin" => Ok(AdminUser(u)),
                _ => Err(AppError::Forbidden.into_response()),
            },
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}

// ── LC-72: scoped API tokens ────────────────────────────────────────────

/// Hex HMAC-SHA256 of a token plaintext, keyed by the server secret. Only
/// this hash is ever stored or compared; the plaintext is never persisted
/// or logged.
pub fn hash_api_token(secret: &[u8; 32], token: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Generate a fresh token plaintext: a `lc_` prefix plus 48 url-safe
/// alphanumerics. Shown to the user exactly once.
pub fn generate_api_token() -> String {
    use rand::Rng;
    // LC-155: use the OS CSPRNG explicitly. `thread_rng` is currently
    // CSPRNG-backed too, but for a security token (also reused as the
    // incoming-webhook secret) the guarantee should be in the source, not
    // incidental to the default RNG.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rngs::OsRng;
    let body: String = (0..48)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect();
    format!("lc_{body}")
}

fn token_is_expired(expires_at: &Option<String>) -> bool {
    let Some(s) = expires_at else { return false };
    // Stored as "YYYY-MM-DD HH:MM:SS" UTC.
    match chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt.and_utc() <= chrono::Utc::now(),
        // Unparseable expiry: treat as expired rather than valid-forever.
        Err(_) => true,
    }
}

/// Extractor for the JSON API: authenticates a `Authorization: Bearer
/// <token>` request to a `User` plus the token's scope set. Returns 401 for
/// a missing / unknown / revoked / expired token (or when the deployment has
/// no secret key, which disables the API). Scope enforcement is per-route
/// via [`ApiAuth::require`], which returns 403 - never 401 - for a valid
/// token that lacks the scope.
pub struct ApiAuth {
    pub user: User,
    pub scopes: std::collections::HashSet<String>,
}

impl ApiAuth {
    /// Refuse (403) when the token lacks `scope`.
    pub fn require(&self, scope: &str) -> Result<(), AppError> {
        if self.scopes.contains(scope) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    /// LC-78: refuse (403) when the authenticated user is a bridge-role
    /// bot. Apply at the top of every endpoint OUTSIDE
    /// `/api/v1/bridges/*` so a bridge bot's blast radius is exactly
    /// bridge-post + heartbeat. Belt-and-braces on top of scope gating:
    /// even if an operator mistakenly grants `messages:write` to a
    /// bridge-role token, this check denies the call.
    pub fn require_not_bridge(&self) -> Result<(), AppError> {
        if self.user.role == crate::db::auth::ROLE_BRIDGE {
            Err(AppError::Forbidden)
        } else {
            Ok(())
        }
    }
}

impl FromRequestParts<AppState> for ApiAuth {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let unauthorized = || AppError::Unauthorized.into_response();

        // API tokens require a configured secret to key the HMAC.
        let Some(secret) = state.secret_key.as_ref() else {
            return Err(unauthorized());
        };
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            // RFC 7235: the auth scheme is case-insensitive ("Bearer" /
            // "bearer" / "BEARER").
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, rest)| rest.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(unauthorized)?;

        let hash = hash_api_token(secret, token);
        let row = match db::api_tokens::find_by_hash(&state.auth, &hash).await {
            Ok(Some(r)) => r,
            Ok(None) => return Err(unauthorized()),
            Err(_) => return Err(unauthorized()),
        };
        if row.revoked_at.is_some() || token_is_expired(&row.expires_at) {
            return Err(unauthorized());
        }
        let record = match db::auth::find_user_by_id(&state.auth, &row.user_id).await {
            Ok(Some(u)) => u,
            _ => return Err(unauthorized()),
        };

        // Best-effort `last_used_at` bump; detached so it never blocks.
        let pool = state.auth.clone();
        let id = row.id;
        tokio::spawn(async move {
            let _ = db::api_tokens::touch_last_used(&pool, id).await;
        });

        let scopes = row
            .scopes
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        Ok(ApiAuth {
            user: record.into(),
            scopes,
        })
    }
}
