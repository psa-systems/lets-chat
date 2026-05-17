pub mod admin;
pub mod admin_sso;
pub mod auth;
pub mod bookmarks;
pub mod dm;
pub mod dm_header;
pub mod email_auth;
pub mod email_digest;
pub mod enclave;
pub mod home;
pub mod layout;
pub mod login_alert;
pub mod markdown;
pub mod mentions;
pub mod not_found;
pub mod notify_prefs;
pub mod pinned;
pub mod room;
pub mod search;
pub mod settings;
pub mod two_factor;
pub mod users;
pub mod voice;
pub mod ws_fragments;

use askama::Template;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// Pre-rendered HTML body. Use `html(&template)` to construct.
pub struct Html(pub String);

impl IntoResponse for Html {
    fn into_response(self) -> Response {
        ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], self.0).into_response()
    }
}

/// Render an Askama template to a `String`, deferring to
/// `tokio::task::block_in_place` on a multi-thread runtime so the
/// synchronous render (which includes syntect-based code-block
/// highlighting via `body_html()`) does not pin a worker thread for
/// seconds at a time and starve every other task on it.
///
/// Concurrent room and WS-fragment renders would otherwise pin all
/// worker threads at once, leaving no thread free to poll incoming
/// requests; that was the source of the multi-minute "page is loading"
/// stalls.
///
/// Falls back to inline render on the current-thread runtime used by
/// `#[tokio::test]`, where `block_in_place` would panic.
pub fn render_template<T: Template>(template: &T) -> Result<String, askama::Error> {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(h) if matches!(h.runtime_flavor(), RuntimeFlavor::MultiThread) => {
            tokio::task::block_in_place(|| template.render())
        }
        _ => template.render(),
    }
}

/// Render an Askama template into an `Html` response wrapper.
pub fn html<T: Template>(template: &T) -> Result<Html, AppError> {
    Ok(Html(render_template(template)?))
}
