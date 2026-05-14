use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::db;
use crate::error::AppError;
use crate::state::AppState;

pub async fn get_avatar(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Result<Response, AppError> {
    let user = db::auth::find_user_by_id(&state.auth, &user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let Some(ext) = user.avatar_ext else {
        // No custom avatar: serve a generated initial-on-color SVG so
        // `/avatars/{id}` always resolves to an image (used by the voice
        // grid and DM call overlay, which can't branch on avatar presence).
        return Ok(default_avatar(&user.username));
    };
    let path = db::avatars_dir().join(format!("{}.{}", user_id, ext));
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        bytes,
    )
        .into_response())
}

/// Generated fallback avatar: the user's first initial on a flat slate
/// background, mirroring the CSS default in `partials/avatar.html`. Returned
/// as an SVG so it scales cleanly in every avatar slot.
fn default_avatar(username: &str) -> Response {
    let initial = username
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"128\" height=\"128\" \
         viewBox=\"0 0 128 128\"><rect width=\"128\" height=\"128\" fill=\"#cbd5e1\"/>\
         <text x=\"64\" y=\"64\" font-family=\"sans-serif\" font-size=\"60\" font-weight=\"600\" \
         fill=\"#334155\" text-anchor=\"middle\" dominant-baseline=\"central\">{initial}</text></svg>"
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        svg,
    )
        .into_response()
}
