//! LC-541: developer-only theme/component gallery.
//!
//! Renders every shared component across all six palettes x four modes so a
//! human can eyeball the token contract (surface/content/border/accent/state
//! colors) after a palette edit. Gated to debug builds via
//! `cfg!(debug_assertions)`; a release binary answers 404, so this is never a
//! user-facing route even though it is registered unconditionally.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};

use crate::state::AppState;

#[derive(Template)]
#[template(path = "dev/theme_gallery.html")]
struct ThemeGallery {
    palettes: Vec<&'static str>,
    modes: Vec<&'static str>,
}

/// GET /dev/theme-gallery - renders every shared component across all palettes x
/// modes. Gated to debug builds; never a user-facing route.
pub async fn theme_gallery(State(_state): State<AppState>) -> Response {
    if !cfg!(debug_assertions) {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let tpl = ThemeGallery {
        palettes: vec![
            "blue-harbor",
            "cobalt",
            "ink-ice",
            "arctic",
            "deep-sea",
            "royal-navy",
        ],
        modes: vec!["light", "dark", "hc-light", "hc-dark"],
    };
    Html(tpl.render().unwrap_or_default()).into_response()
}
