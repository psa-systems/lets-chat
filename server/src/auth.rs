use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;
use dashmap::DashMap;

use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

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
pub fn extract_session_origin(headers: &axum::http::HeaderMap) -> (Option<String>, Option<String>) {
    fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    let ua = header_str(headers, "user-agent").map(|s| truncate(&s, 256));
    let ip = header_str(headers, "x-forwarded-for")
        .and_then(|s| s.split(',').next().map(|p| p.trim().to_string()))
        .filter(|s| !s.is_empty())
        .or_else(|| header_str(headers, "x-real-ip"))
        .map(|s| truncate(&s, 64));
    (ua, ip)
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
            let pool = state.auth.clone();
            let id = t.clone();
            tokio::spawn(async move {
                let _ = db::auth::touch_session_last_seen(&pool, &id).await;
            });
        }
        req.extensions_mut().insert(CurrentSessionId(t));
    }
    next.run(req).await
}

fn should_touch_last_seen(ledger: &LastSeenLedger, session_id: &str) -> bool {
    let now = Instant::now();
    match ledger.get(session_id) {
        Some(prev) if now.duration_since(*prev) < LAST_SEEN_DEBOUNCE => false,
        _ => {
            ledger.insert(session_id.to_string(), now);
            true
        }
    }
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

/// Middleware: redirect any authenticated user without 2FA enabled to the
/// enrollment page, with carve-outs for the auth flow itself, the enrollment
/// page, and static assets so the user can actually complete setup. No-op
/// when the deployment has no `LETS_CHAT_SECRET_KEY` configured, which
/// disables 2FA entirely.
pub async fn enforce_2fa_enrollment(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if !state.two_factor_available() {
        return next.run(req).await;
    }
    let path = req.uri().path();
    let exempt = path == "/logout"
        || path == "/login"
        || path.starts_with("/login/")
        || path == "/register"
        || path == "/forgot"
        || path.starts_with("/reset/")
        || path == "/settings/2fa/setup"
        || path == "/version"
        || path.starts_with("/assets/")
        || path.starts_with("/avatars/");
    if !exempt {
        if let Some(u) = req.extensions().get::<User>() {
            if !u.totp_enabled {
                return Redirect::to("/settings/2fa/setup").into_response();
            }
        }
    }
    next.run(req).await
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
