use askama::Template;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use thiserror::Error;

use crate::i18n;
use crate::views::error_page::ErrorPage;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    /// `(message, retry_after_seconds)`. The seconds value is rendered
    /// into the `Retry-After` HTTP header so well-behaved clients back
    /// off automatically; the message goes in the response body for
    /// the operator.
    #[error("too many requests: {0}")]
    TooManyRequests(String, u64),
    #[error("unauthorized")]
    Unauthorized,
    #[error("internal: {0}")]
    Internal(String),
    #[error("redirect")]
    Redirect(Redirect),
}

/// LC-220: maximum length of the dynamic `msg` rendered on the error page.
/// `BadRequest` / `Conflict` / `PayloadTooLarge` callers sometimes
/// interpolate user-supplied strings (e.g. `format!("invalid mute_mode: {}",
/// form.mute_mode)`); Askama auto-escapes HTML so XSS is prevented, but
/// clamping the length limits same-session content reflection (a phishing
/// payload squeezed into a query param cannot grow arbitrarily large).
const MAX_MESSAGE_LEN: usize = 200;

/// LC-220: render the styled error page or fall back to bare text + the raw
/// status code if Askama somehow fails. Used by `AppError::IntoResponse`
/// for every variant except `Redirect`. The `back_url` is hardcoded to `/`;
/// the auth middleware redirects unauthed visitors to `/login` from there,
/// so a single value works for both cases.
fn render_styled(status: StatusCode, heading_key: &str, message: Option<&str>) -> Response {
    // Heading is localized through Fluent (CURRENT_LOCALE task-local is set
    // by resolve_locale middleware before the handler runs, so it is still
    // in scope here when IntoResponse fires).
    let heading = i18n::translate_current(heading_key);
    let back_label = i18n::translate_current("not-found-back-home");
    // Clamp the dynamic message to MAX_MESSAGE_LEN chars (NOT bytes) so a
    // long error string from a foreign-language locale or a deeply nested
    // serde / sqlx error display does not blow out the panel. Truncation
    // happens AFTER any escaping handled by Askama at render time.
    let clamped: Option<String> = message.map(|m| {
        if m.chars().count() > MAX_MESSAGE_LEN {
            m.chars().take(MAX_MESSAGE_LEN).collect::<String>() + "…"
        } else {
            m.to_string()
        }
    });
    let page = ErrorPage {
        status: status.as_u16(),
        status_heading: &heading,
        message: clamped.as_deref(),
        back_url: "/",
        back_label: &back_label,
        asset_version: "",
    };
    match page.render() {
        Ok(body) => (
            status,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to render error page");
            (status, heading).into_response()
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => {
                render_styled(StatusCode::NOT_FOUND, "error-status-not-found", None)
            }
            AppError::Forbidden => {
                render_styled(StatusCode::FORBIDDEN, "error-status-forbidden", None)
            }
            AppError::Conflict(msg) => {
                render_styled(StatusCode::CONFLICT, "error-status-conflict", Some(&msg))
            }
            AppError::BadRequest(msg) => render_styled(
                StatusCode::BAD_REQUEST,
                "error-status-bad-request",
                Some(&msg),
            ),
            AppError::PayloadTooLarge(msg) => render_styled(
                StatusCode::PAYLOAD_TOO_LARGE,
                "error-status-payload-too-large",
                Some(&msg),
            ),
            AppError::TooManyRequests(msg, retry_after) => {
                // Render the styled page, then add the Retry-After header
                // so well-behaved clients still back off.
                let mut resp = render_styled(
                    StatusCode::TOO_MANY_REQUESTS,
                    "error-status-too-many-requests",
                    Some(&msg),
                );
                if let Ok(v) = axum::http::HeaderValue::from_str(&retry_after.to_string()) {
                    resp.headers_mut()
                        .insert(axum::http::header::RETRY_AFTER, v);
                }
                resp
            }
            AppError::Unauthorized => {
                render_styled(StatusCode::UNAUTHORIZED, "error-status-unauthorized", None)
            }
            AppError::Internal(msg) => {
                // LC-220: log the underlying error for the operator but NEVER
                // expose `msg` to the client. The styled page shows only the
                // generic localized heading so a sqlx / askama / panic-trace
                // message cannot leak into the response body.
                tracing::error!(error = %msg, "internal error");
                render_styled(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "error-status-internal",
                    None,
                )
            }
            AppError::Redirect(r) => r.into_response(),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(format!("sqlx: {e}"))
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Internal(format!("askama: {e}"))
    }
}
