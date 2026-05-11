use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::perms::enclave_can_manage;
use crate::state::AppState;

/// Hard cap for a single custom emoji file. Kept small so the picker stays
/// snappy and bandwidth costs stay bounded.
const MAX_EMOJI_BYTES: i64 = 256 * 1024;

fn allowed_ext_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// Shortcodes are lowercased ASCII, digits, and underscores, 2..=32 chars.
/// The constraint matches the parser in `crate::render::render_body` so
/// every legal token can be typed in a message and resolved at render time.
fn is_valid_shortcode(s: &str) -> bool {
    let len = s.chars().count();
    if !(2..=32).contains(&len) {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

async fn require_manage(
    state: &AppState,
    user: &crate::models::User,
    enclave_id: i64,
) -> Result<(), AppError> {
    let m = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
    if !enclave_can_manage(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// POST /enclave/{id}/emojis - multipart upload. Fields: `shortcode`,
/// `file`. Streams `file` to a tmp path, sniffs magic bytes, content-
/// addresses into `uploads_dir()`, then inserts the `custom_emojis` row.
/// Requires `enclave_can_manage` (owner, admin, or site admin).
pub async fn post_upload(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(enclave_id): Path<i64>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    require_manage(&state, &user, enclave_id).await?;

    let uploads_root = db::uploads_dir();
    let tmp_dir = uploads_root.join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| AppError::Internal(format!("create tmp dir: {e}")))?;

    let mut shortcode: Option<String> = None;
    let mut emoji_payload: Option<(PathBuf, i64)> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        match field.name() {
            Some("shortcode") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("shortcode: {e}")))?;
                let s = String::from_utf8(bytes.to_vec())
                    .map_err(|_| AppError::BadRequest("shortcode must be UTF-8".into()))?;
                shortcode = Some(s.trim().to_ascii_lowercase());
            }
            Some("file") => {
                let tmp_name = format!("{}", Uuid::new_v4());
                let tmp_path = tmp_dir.join(&tmp_name);
                let mut file = File::create(&tmp_path)
                    .await
                    .map_err(|e| AppError::Internal(format!("create tmp file: {e}")))?;
                let mut total: i64 = 0;
                let mut overflow = false;
                while let Some(chunk) = field.next().await {
                    let chunk = match chunk {
                        Ok(c) => c,
                        Err(e) => {
                            drop(file);
                            let _ = tokio::fs::remove_file(&tmp_path).await;
                            return Err(AppError::BadRequest(format!("multipart read: {e}")));
                        }
                    };
                    total = total.saturating_add(chunk.len() as i64);
                    if total > MAX_EMOJI_BYTES {
                        overflow = true;
                        break;
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        drop(file);
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return Err(AppError::Internal(format!("write tmp file: {e}")));
                    }
                }
                if let Err(e) = file.flush().await {
                    drop(file);
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Err(AppError::Internal(format!("flush tmp file: {e}")));
                }
                drop(file);
                if overflow {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Ok((
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!("emoji exceeds {MAX_EMOJI_BYTES}-byte limit"),
                    )
                        .into_response());
                }
                emoji_payload = Some((tmp_path, total));
            }
            _ => continue,
        }
    }

    let Some(shortcode) = shortcode else {
        return Err(AppError::BadRequest("shortcode required".into()));
    };
    if !is_valid_shortcode(&shortcode) {
        return Err(AppError::BadRequest(
            "shortcode must be 2-32 chars of lowercase a-z, 0-9, underscore".into(),
        ));
    }
    let Some((tmp_path, total)) = emoji_payload else {
        return Err(AppError::BadRequest("file required".into()));
    };

    let kind = match infer::get_from_path(&tmp_path) {
        Ok(Some(k)) => k,
        Ok(None) | Err(_) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Ok((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "could not determine file type",
            )
                .into_response());
        }
    };
    let mime_type = kind.mime_type().to_string();
    let Some(ext) = allowed_ext_for_mime(&mime_type) else {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Ok((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("unsupported emoji type: {mime_type}"),
        )
            .into_response());
    };

    let hex = sha256_file(&tmp_path)
        .await
        .map_err(|e| AppError::Internal(format!("hash tmp file: {e}")))?;
    let storage_name = format!("{hex}.{ext}");
    let final_path = uploads_root.join(&storage_name);
    match tokio::fs::metadata(&final_path).await {
        Ok(_) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
        }
        Err(_) => {
            tokio::fs::rename(&tmp_path, &final_path)
                .await
                .map_err(|e| AppError::Internal(format!("rename tmp file: {e}")))?;
        }
    }

    match db::custom_emojis::insert(
        &state.chat,
        enclave_id,
        &shortcode,
        &storage_name,
        &mime_type,
        total,
        &user.id,
    )
    .await
    {
        Ok(_) => {}
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
            return Ok((
                StatusCode::CONFLICT,
                format!("shortcode :{shortcode}: already exists in this enclave"),
            )
                .into_response());
        }
        Err(e) => return Err(e.into()),
    }

    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")).into_response())
}

/// DELETE /enclave/{id}/emojis/{eid} - remove the row. The file on disk is
/// content-addressed and may be referenced by other emojis (or other
/// uploads), so we never delete the file here; orphan GC handles that
/// elsewhere.
pub async fn post_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, emoji_id)): Path<(i64, i64)>,
) -> Result<Response, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    let row = db::custom_emojis::get(&state.chat, emoji_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if row.enclave_id != enclave_id {
        return Err(AppError::NotFound);
    }
    db::custom_emojis::delete(&state.chat, emoji_id).await?;
    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")).into_response())
}

/// GET /api/emojis/{id} - stream a custom emoji file. Any logged-in user
/// may fetch when the emoji's enclave has opted into global sharing;
/// otherwise the caller must be a member of that enclave (or a site
/// admin). Cached aggressively because the id maps to a content-addressed
/// file (immutable for the row's lifetime).
pub async fn get_emoji(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(emoji_id): Path<i64>,
) -> Result<Response, AppError> {
    let row = db::custom_emojis::get(&state.chat, emoji_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let enclave = db::enclave::get_enclave(&state.chat, row.enclave_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_site_admin = user.role == "admin";
    let allowed = is_site_admin
        || enclave.share_emojis_globally
        || db::enclave::get_membership(&state.chat, row.enclave_id, &user.id)
            .await?
            .is_some();
    if !allowed {
        return Err(AppError::Forbidden);
    }

    let path: PathBuf = db::uploads_dir().join(&row.storage_path);
    let file = File::open(&path)
        .await
        .map_err(|_| AppError::Internal("emoji file missing on disk".into()))?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, row.mime_type.clone()),
            (header::CONTENT_LENGTH, row.size_bytes.to_string()),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_string(),
            ),
        ],
        body,
    )
        .into_response())
}

async fn sha256_file(path: &PathBuf) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcode_validates_length_and_charset() {
        assert!(is_valid_shortcode("ok"));
        assert!(is_valid_shortcode("party_parrot_42"));
        assert!(!is_valid_shortcode("a"));
        assert!(!is_valid_shortcode(""));
        assert!(!is_valid_shortcode("UPPER"));
        assert!(!is_valid_shortcode("with-dash"));
        assert!(!is_valid_shortcode("with space"));
        assert!(!is_valid_shortcode(&"a".repeat(33)));
    }
}
