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

/// LC-220: maximum length of the caller-supplied `detail` rendered on the error
/// page. `BadRequest` / `Conflict` / `PayloadTooLarge` callers sometimes
/// interpolate user-supplied strings (e.g. `format!("invalid mute_mode: {}",
/// form.mute_mode)`); Askama auto-escapes HTML so XSS is prevented, but clamping
/// the length limits same-session content reflection.
const MAX_DETAIL_LEN: usize = 200;

/// LC-220 / LC-552: render the styled error page, or fall back to bare text +
/// the raw status code if Askama somehow fails. Used by `AppError::IntoResponse`
/// for every variant except `Redirect`. The `back_url` is hardcoded to `/`;
/// the auth middleware redirects unauthed visitors to `/login` from there, so a
/// single value works for both cases.
///
/// LC-552: the page always shows a friendly, localized `description` keyed by
/// the error variant (so even a bare 404 / 403 now reads as human copy, not an
/// empty heading). `detail` is the caller's specific, curated reason - shown as
/// a secondary line when present (e.g. "Pin cap reached (max 50)"), because
/// hiding it would drop genuinely helpful validation feedback. The truly
/// internal variant (`Internal`, carrying sqlx / askama / panic text) passes
/// `None` here, so operator-facing detail never reaches the client. Escaped by
/// Askama at render time and clamped to [`MAX_DETAIL_LEN`].
fn render_styled(
    status: StatusCode,
    heading_key: &str,
    description_key: &str,
    detail: Option<&str>,
) -> Response {
    // Heading + description are localized through Fluent (CURRENT_LOCALE
    // task-local is set by resolve_locale middleware before the handler runs,
    // so it is still in scope here when IntoResponse fires).
    let heading = i18n::translate_current(heading_key);
    let description = i18n::translate_current(description_key);
    let back_label = i18n::translate_current("not-found-back-home");
    let clamped: Option<String> = detail.map(|m| {
        if m.chars().count() > MAX_DETAIL_LEN {
            m.chars().take(MAX_DETAIL_LEN).collect::<String>() + "…"
        } else {
            m.to_string()
        }
    });
    let page = ErrorPage {
        status: status.as_u16(),
        status_heading: &heading,
        description: &description,
        detail: clamped.as_deref(),
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
            AppError::NotFound => render_styled(
                StatusCode::NOT_FOUND,
                "error-status-not-found",
                "error-desc-not-found",
                None,
            ),
            AppError::Forbidden => render_styled(
                StatusCode::FORBIDDEN,
                "error-status-forbidden",
                "error-desc-forbidden",
                None,
            ),
            AppError::Conflict(msg) => render_styled(
                StatusCode::CONFLICT,
                "error-status-conflict",
                "error-desc-conflict",
                Some(&msg),
            ),
            AppError::BadRequest(msg) => render_styled(
                StatusCode::BAD_REQUEST,
                "error-status-bad-request",
                "error-desc-bad-request",
                Some(&msg),
            ),
            AppError::PayloadTooLarge(msg) => render_styled(
                StatusCode::PAYLOAD_TOO_LARGE,
                "error-status-payload-too-large",
                "error-desc-payload-too-large",
                Some(&msg),
            ),
            AppError::TooManyRequests(msg, retry_after) => {
                // Render the styled page, then add the Retry-After header
                // so well-behaved clients still back off.
                let mut resp = render_styled(
                    StatusCode::TOO_MANY_REQUESTS,
                    "error-status-too-many-requests",
                    "error-desc-too-many-requests",
                    Some(&msg),
                );
                if let Ok(v) = axum::http::HeaderValue::from_str(&retry_after.to_string()) {
                    resp.headers_mut()
                        .insert(axum::http::header::RETRY_AFTER, v);
                }
                resp
            }
            AppError::Unauthorized => render_styled(
                StatusCode::UNAUTHORIZED,
                "error-status-unauthorized",
                "error-desc-unauthorized",
                None,
            ),
            AppError::Internal(msg) => {
                // LC-220 / LC-552: log the underlying error for the operator but
                // NEVER expose `msg` to the client. `detail = None` so a sqlx /
                // askama / panic-trace message cannot leak into the response
                // body; the user sees only the friendly generic description.
                tracing::error!(error = %msg, "internal error");
                render_styled(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "error-status-internal",
                    "error-desc-internal",
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

#[cfg(test)]
mod tests {
    use super::AppError;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    async fn body_string(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    // LC-552: a client-error variant shows BOTH the friendly per-status copy
    // and the caller's curated reason (helpful validation feedback).
    #[tokio::test]
    async fn bad_request_shows_friendly_copy_and_curated_detail() {
        let resp = AppError::BadRequest("Pin cap reached (max 50)".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        assert!(
            body.contains("could not process that request"),
            "friendly bad-request description missing: {body}"
        );
        assert!(
            body.contains("Pin cap reached (max 50)"),
            "curated detail should still be shown: {body}"
        );
    }

    // LC-552: the truly-internal variant NEVER leaks its sqlx / host / panic
    // text - that is the security-relevant guarantee.
    #[tokio::test]
    async fn internal_error_never_leaks_its_message() {
        let resp =
            AppError::Internal("sqlx: connection refused at 10.0.0.5".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_string(resp).await;
        assert!(
            !body.contains("10.0.0.5") && !body.contains("sqlx"),
            "internal error detail leaked into the error page"
        );
        assert!(body.contains("Something went wrong on our end"));
    }

    // LC-552: a bare no-detail variant still gets human copy instead of an
    // empty heading.
    #[tokio::test]
    async fn not_found_renders_friendly_description() {
        let resp = AppError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_string(resp).await;
        assert!(body.contains("could not find that page"), "{body}");
    }

    #[tokio::test]
    async fn too_many_requests_sets_retry_after_and_shows_detail() {
        let resp = AppError::TooManyRequests("retry in 30 seconds".into(), 30).into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("30")
        );
        let body = body_string(resp).await;
        assert!(
            body.contains("too quickly"),
            "friendly copy missing: {body}"
        );
        assert!(
            body.contains("retry in 30 seconds"),
            "curated retry detail should be shown: {body}"
        );
    }
}
