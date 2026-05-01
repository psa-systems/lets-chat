use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;

use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "session";

/// Middleware: read the session cookie, look up the user, and inject the
/// resulting `User` into request extensions when the session is valid and the
/// account is not banned.
pub async fn inject_user(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let user = match jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) {
        Some(token) => match db::auth::get_user_by_session(&state.auth, &token).await {
            Ok(Some(record)) if !record.is_banned => Some(User::from(record)),
            _ => None,
        },
        None => None,
    };
    if let Some(u) = user {
        req.extensions_mut().insert(u);
    }
    next.run(req).await
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
