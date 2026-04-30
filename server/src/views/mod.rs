pub mod auth;
pub mod dm;
pub mod home;
pub mod layout;
pub mod room;
pub mod search;
pub mod ws_fragments;

use askama::Template;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// Pre-rendered HTML body. Use `html(&template)` to construct.
pub struct Html(pub String);

impl IntoResponse for Html {
    fn into_response(self) -> Response {
        (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            self.0,
        )
            .into_response()
    }
}

/// Render an Askama template into an `Html` response wrapper.
pub fn html<T: Template>(template: &T) -> Result<Html, AppError> {
    Ok(Html(template.render()?))
}
