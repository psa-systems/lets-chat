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
///
/// Each phase is wrapped in a watchdog: a sibling task fires a warn log
/// the moment the phase exceeds 3 s, regardless of whether it ever
/// completes. After-the-fact logging is useless for hangs: the warn
/// only fires once the await returns, so a phase stuck forever would
/// produce no output at all without the watchdog.
pub async fn inject_user(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    const PHASE_WATCHDOG: Duration = Duration::from_secs(1);

    let path = req.uri().path().to_string();
    tracing::debug!(%path, "inject_user enter");

    // Independent watchdog spawned at request start so we can distinguish
    // "inject_user task suspended" from "phase took longer than 5 s but
    // task is still being polled". An in-task tokio::select! sleep only
    // fires when the task itself gets polled; this one runs as its own
    // task and fires regardless of the parent.
    let done = Arc::new(AtomicBool::new(false));
    {
        let done = done.clone();
        let path = path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if !done.load(Ordering::SeqCst) {
                tracing::warn!(
                    %path,
                    "inject_user OUTER watchdog: task suspended for 5s (still inside inject_user)"
                );
            }
        });
    }
    let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());

    let lookup_start = Instant::now();
    let user = {
        let path = path.clone();
        let fut = async {
            match token.as_deref() {
                Some(t) => match db::auth::get_user_by_session(&state.auth, t).await {
                    Ok(Some(record)) if !record.is_banned => Some(User::from(record)),
                    _ => None,
                },
                None => None,
            }
        };
        tokio::pin!(fut);
        loop {
            tokio::select! {
                result = &mut fut => break result,
                _ = tokio::time::sleep(PHASE_WATCHDOG) => {
                    tracing::warn!(
                        %path,
                        elapsed_ms = lookup_start.elapsed().as_millis() as u64,
                        "inject_user phase still running: session_lookup",
                    );
                }
            }
        }
    };
    tracing::info!(
        %path,
        elapsed_ms = lookup_start.elapsed().as_millis() as u64,
        "inject_user phase done: session_lookup",
    );

    if let (Some(u), Some(t)) = (user, token) {
        req.extensions_mut().insert(u);
        if should_touch_last_seen(&state.last_seen_ledger, &t) {
            state.bg.touch_session(&t);
        }
        req.extensions_mut().insert(CurrentSessionId(t));
    }
    tracing::debug!(%path, "inject_user: post-lookup setup done, entering downstream loop");

    let downstream_start = Instant::now();
    let fut = next.run(req);
    tokio::pin!(fut);
    tracing::debug!(%path, "inject_user: downstream fut pinned, about to poll");
    let resp = loop {
        tokio::select! {
            result = &mut fut => break result,
            _ = tokio::time::sleep(PHASE_WATCHDOG) => {
                tracing::warn!(
                    %path,
                    elapsed_ms = downstream_start.elapsed().as_millis() as u64,
                    "inject_user phase still running: downstream_handler",
                );
            }
        }
    };
    tracing::info!(
        %path,
        elapsed_ms = downstream_start.elapsed().as_millis() as u64,
        "inject_user phase done: downstream_handler",
    );
    done.store(true, Ordering::SeqCst);
    resp
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
    let path = req.uri().path().to_string();
    tracing::debug!(%path, "enforce_2fa enter");
    if !state.two_factor_available() {
        let r = next.run(req).await;
        tracing::debug!(%path, "enforce_2fa: next.run done (no 2fa)");
        return r;
    }
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
                tracing::debug!(%path, "enforce_2fa: redirecting to 2fa setup");
                return Redirect::to("/settings/2fa/setup").into_response();
            }
        }
    }
    tracing::debug!(%path, "enforce_2fa: passing through to next.run");
    let r = next.run(req).await;
    tracing::debug!(%path, "enforce_2fa: next.run done");
    r
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
