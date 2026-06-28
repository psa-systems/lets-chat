//! LC-488: GIF picker routes.
//!
//! - `GET /api/gifs?q=` searches Giphy (trending when `q` is empty) and renders
//!   a result grid.
//! - `POST /room/{id}/gif` fetches the chosen GIF from the Giphy CDN, stores it
//!   through the uploads pipeline (same-origin), and posts it as a message
//!   attachment.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::gif::{self, GifConfig};
use crate::state::AppState;
use crate::views::gif::GifResultsFragment;
use crate::views::{html, Html};

const SEARCH_LIMIT: u32 = 24;
/// Hard cap on a fetched GIF (Giphy "downsized" GIFs are well under this).
const MAX_GIF_BYTES: usize = 8 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
}

/// GET /api/gifs?q= - render the result grid (featured when q is empty).
pub async fn search(
    State(_state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(SearchQuery { q }): Query<SearchQuery>,
) -> Result<Html, AppError> {
    let Some(cfg) = GifConfig::from_env() else {
        return Err(AppError::BadRequest("GIF picker is not configured".into()));
    };
    let q = q.trim();
    // Giphy: /trending for an empty query, /search otherwise.
    let endpoint = if q.is_empty() { "trending" } else { "search" };
    let mut url = url::Url::parse(&format!("https://api.giphy.com/v1/gifs/{endpoint}"))
        .map_err(|e| AppError::Internal(format!("giphy url: {e}")))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("api_key", &cfg.api_key)
            .append_pair("rating", &cfg.rating)
            .append_pair("limit", &SEARCH_LIMIT.to_string());
        if !q.is_empty() {
            qp.append_pair("q", q);
        }
    }
    let resp = crate::http_client::outbound_get(url.as_str())
        .await
        .map_err(|e| AppError::BadRequest(format!("gif search: {e}")))?
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "giphy search failed");
            AppError::BadRequest("gif search failed".into())
        })?;
    if !resp.status().is_success() {
        return Err(AppError::BadRequest("gif search failed".into()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::BadRequest(format!("gif search parse: {e}")))?;
    let results = gif::parse_search(&body);
    html(&GifResultsFragment { results: &results })
}

#[derive(Deserialize)]
pub struct PostGifForm {
    pub url: String,
}

/// POST /room/{room_id}/gif - fetch the chosen Giphy GIF, store it same-origin,
/// and post it as a message attachment (empty body, like an image-only post).
pub async fn post_gif(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<PostGifForm>,
) -> Result<Response, AppError> {
    if GifConfig::from_env().is_none() {
        return Err(AppError::BadRequest("GIF picker is not configured".into()));
    }
    // Mirror the poll/event posting gate.
    if user.is_banned || user.is_muted {
        return Err(AppError::Forbidden);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    // Only ever fetch a Giphy CDN URL (plus the http_client SSRF filter).
    if !gif::is_giphy_media_url(&form.url) {
        return Err(AppError::BadRequest("not a Giphy GIF URL".into()));
    }

    let file_id = fetch_and_store_gif(&state, &user.id, &form.url).await?;

    // Post as an image-only message: empty body + the stored attachment.
    let new_id = db::chat::insert_message(&state.chat, room.id, &user.id, "").await?;
    db::uploads::link_upload_to_message(&state.chat, file_id, new_id).await?;
    crate::routes::room::finalize_message_send(&state, &room, &user, new_id, "", None).await?;
    Ok((axum::http::StatusCode::OK, "").into_response())
}

/// Fetch `url` (already host-allowlisted), stream it with a hard byte cap,
/// sniff + re-encode through the uploads pipeline (LC-206 decode limits), and
/// store a content-addressed `file_uploads` row. Returns the new file id.
async fn fetch_and_store_gif(
    state: &AppState,
    uploader_id: &str,
    url: &str,
) -> Result<i64, AppError> {
    let resp = crate::http_client::outbound_get(url)
        .await
        .map_err(|e| AppError::BadRequest(format!("gif fetch: {e}")))?
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "gif fetch failed");
            AppError::BadRequest("gif fetch failed".into())
        })?;
    if !resp.status().is_success() {
        return Err(AppError::BadRequest("gif fetch failed".into()));
    }

    // Streamed read with a hard cap; Content-Length is advisory.
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut stream = resp;
    loop {
        match stream.chunk().await {
            Ok(Some(c)) => {
                if buf.len() + c.len() > MAX_GIF_BYTES {
                    return Err(AppError::BadRequest("GIF is too large".into()));
                }
                buf.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(e) => return Err(AppError::BadRequest(format!("gif read: {e}"))),
        }
    }

    let uploads_root = db::uploads_dir();
    let tmp_path = uploads_root.join(format!(".gif-tmp-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&tmp_path, &buf)
        .await
        .map_err(|e| AppError::Internal(format!("gif temp write: {e}")))?;

    // Magic-byte sniff (the provider Content-Type is ignored); must be a GIF.
    let sniffed = infer::get_from_path(&tmp_path)
        .ok()
        .flatten()
        .map(|t| t.mime_type().to_string());
    if sniffed.as_deref() != Some("image/gif") {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(AppError::BadRequest("fetched file is not a GIF".into()));
    }

    let processed =
        crate::uploads::pipeline::process_image_safely(tmp_path.clone(), "image/gif".to_string())
            .await;
    let _ = tokio::fs::remove_file(&tmp_path).await;
    let processed = processed.map_err(|e| {
        tracing::warn!(error = %e, "gif decode failed");
        AppError::BadRequest("could not process GIF".into())
    })?;

    // Content-address + store original + best-effort preview (mirrors post_upload).
    let hex = crate::uploads::sha256_bytes(&processed.original_bytes);
    let storage_name = format!("{hex}.gif");
    let final_path = uploads_root.join(&storage_name);
    let preview_name = crate::uploads::preview_storage_name(&storage_name);
    let preview_path = uploads_root.join(&preview_name);
    if tokio::fs::metadata(&final_path).await.is_err() {
        crate::uploads::write_atomic(&final_path, &processed.original_bytes)
            .await
            .map_err(|e| AppError::Internal(format!("gif write: {e}")))?;
        if let Err(e) = crate::uploads::write_atomic(&preview_path, &processed.preview_bytes).await
        {
            tracing::warn!(error = %e, "gif preview write failed (original committed)");
        }
    } else if tokio::fs::metadata(&preview_path).await.is_err() {
        let _ = crate::uploads::write_atomic(&preview_path, &processed.preview_bytes).await;
    }

    let id = db::uploads::insert_upload(
        &state.chat,
        uploader_id,
        "giphy.gif",
        &processed.mime,
        processed.original_bytes.len() as i64,
        &storage_name,
        None,
    )
    .await?;
    Ok(id)
}
