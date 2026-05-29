//! LC-220: standalone themed error page. Extends `base.html`, not
//! `layout.html`, so the page renders without sidebar chrome (which would
//! require a DB query the `AppError::IntoResponse` path does not have access
//! to). The user can still navigate via the `back_url` link or, on mobile,
//! the LC-199 nav drawer that already exists on every page.

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage<'a> {
    /// Numeric HTTP status code rendered as the small mono prefix.
    pub status: u16,
    /// Localized status heading (e.g. "Not found", "Forbidden", "Server
    /// error"). Plain text, no markup.
    pub status_heading: &'a str,
    /// Optional variant-specific detail. Plain text; HTML-escaped by Askama.
    /// `None` renders the heading + back link only.
    pub message: Option<&'a str>,
    /// Where the "Back" button navigates to. `/` for authed users, `/login`
    /// for unauthed, but the caller decides.
    pub back_url: &'a str,
    /// Localized "Back to home" / "Back to login" label.
    pub back_label: &'a str,
    /// Cache-buster appended to CSS / JS / favicon URLs in `base.html`. We
    /// use an empty string here because `AppError::IntoResponse` has no
    /// access to `AppState`. The browser may serve a slightly-stale asset
    /// for an error page; acceptable trade-off versus threading state
    /// through the IntoResponse path.
    pub asset_version: &'a str,
}
