pub mod admin;
pub mod auth;
pub mod bookmarks;
pub mod dm;
pub mod dm_header;
pub mod enclave;
pub mod home;
pub mod layout;
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

/// Render an Askama template into an `Html` response wrapper.
///
/// On the multi-thread tokio runtime we hand the synchronous render off
/// via `block_in_place`, so the syntect-heavy `body_html()` calls inside
/// the template do not pin a worker thread for seconds at a time and
/// starve every other task scheduled on it. Under chat load several
/// concurrent room renders could otherwise pin all worker threads
/// simultaneously, leaving no thread free to poll incoming requests and
/// producing the multi-minute "page is loading" stalls.
///
/// On the current-thread runtime (used by `#[tokio::test]`) we render
/// inline: `block_in_place` panics there, and the test workload is
/// single-shot anyway.
pub fn html<T: Template>(template: &T) -> Result<Html, AppError> {
    use tokio::runtime::{Handle, RuntimeFlavor};
    let rendered = match Handle::try_current() {
        Ok(h) if matches!(h.runtime_flavor(), RuntimeFlavor::MultiThread) => {
            tokio::task::block_in_place(|| template.render())
        }
        _ => template.render(),
    }?;
    Ok(Html(rendered))
}
