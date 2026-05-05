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
        return Err(AppError::NotFound);
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
