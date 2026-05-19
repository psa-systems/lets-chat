use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use thiserror::Error;

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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Not Found").into_response(),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden").into_response(),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg).into_response(),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            AppError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg).into_response(),
            AppError::TooManyRequests(msg, retry_after) => (
                StatusCode::TOO_MANY_REQUESTS,
                [(axum::http::header::RETRY_AFTER, retry_after.to_string())],
                msg,
            )
                .into_response(),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized").into_response(),
            AppError::Internal(msg) => {
                tracing::error!(error = %msg, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
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
