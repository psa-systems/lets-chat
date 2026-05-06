use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::auth::{AuthUser, OptionalUser};
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// Default upload cap when settings.max_upload_bytes is missing or unparseable.
const DEFAULT_MAX_UPLOAD_BYTES: i64 = 10 * 1024 * 1024;

/// Resolve max upload size from settings.db, falling back to 10 MiB.
async fn max_upload_bytes(state: &AppState) -> i64 {
    db::settings::get_setting(&state.settings, "max_upload_bytes")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_BYTES)
}

/// Map a sniffed `infer::Type` MIME string to the canonical extension we
/// store in `storage_path`. Only allowed types pass through; any other input
/// returns `None` and the caller should reject with 415.
fn allowed_ext_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "application/pdf" => Some("pdf"),
        _ => None,
    }
}

/// `POST /api/upload` - multipart upload endpoint. Streams the first `file`
/// field into a temp file under `${LETS_CHAT_DATA_DIR}/uploads/.tmp/{uuid}`,
/// counting bytes against `max_upload_bytes`. After streaming, sniffs magic
/// bytes via `infer`, validates against the allowlist, computes the sha256,
/// renames to `${LETS_CHAT_DATA_DIR}/uploads/{sha256}.{ext}` (or de-dups if
/// the destination already exists), inserts an orphan row in `file_uploads`
/// and returns `{ "file_id": id, "url": "/api/files/{id}" }`.
pub async fn post_upload(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let max_bytes = max_upload_bytes(&state).await;

    let uploads_root = db::uploads_dir();
    let tmp_dir = uploads_root.join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| AppError::Internal(format!("create tmp dir: {e}")))?;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let original_filename = field
            .file_name()
            .map(sanitize_filename)
            .unwrap_or_else(|| "upload.bin".to_string());

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
            if total > max_bytes {
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
                format!("file exceeds {max_bytes}-byte limit"),
            )
                .into_response());
        }

        // Magic-byte sniffing. Never trust the supplied Content-Type.
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
                format!("unsupported file type: {mime_type}"),
            )
                .into_response());
        };

        // sha256 the temp file, rename to content-addressed path.
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

        let id = db::uploads::insert_upload(
            &state.chat,
            &user.id,
            &original_filename,
            &mime_type,
            total,
            &storage_name,
        )
        .await?;

        return Ok(Json(serde_json::json!({
            "file_id": id,
            "url": format!("/api/files/{id}"),
        }))
        .into_response());
    }

    Err(AppError::BadRequest("missing file field".into()))
}

/// `GET /api/files/{id}` - stream a previously uploaded file. Auth-gated:
/// orphan uploads (`message_id IS NULL`) are visible only to the original
/// uploader; otherwise the same `is_room_accessible` predicate used by
/// message handlers applies. 401 for anonymous, 403 for authed-but-no-access.
pub async fn get_file(
    State(state): State<AppState>,
    OptionalUser(maybe_user): OptionalUser,
    Path(file_id): Path<i64>,
) -> Result<Response, AppError> {
    let Some(user) = maybe_user else {
        return Ok((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    };

    let (row, room_id) = match db::uploads::get_upload(&state.chat, file_id).await? {
        Some(t) => t,
        None => return Err(AppError::NotFound),
    };

    let allowed = if let Some(rid) = room_id {
        let is_admin = user.role == "admin";
        db::chat::is_room_accessible(&state.chat, rid, &user.id, is_admin).await?
    } else {
        // Orphan upload: only the original uploader may fetch.
        row.uploader_id == user.id
    };
    if !allowed {
        return Err(AppError::Forbidden);
    }

    let path: PathBuf = db::uploads_dir().join(&row.storage_path);
    let file = File::open(&path)
        .await
        .map_err(|_| AppError::Internal("upload file missing on disk".into()))?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let disposition = format!(
        "inline; filename=\"{}\"",
        sanitize_disposition_filename(&row.filename)
    );

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, row.mime_type.clone()),
            (header::CONTENT_DISPOSITION, disposition),
            (
                header::CONTENT_LENGTH,
                row.size_bytes.to_string(),
            ),
            (
                header::CACHE_CONTROL,
                "private, max-age=86400".to_string(),
            ),
        ],
        body,
    )
        .into_response())
}

/// Sha256 a file by streaming through it 64 KiB at a time. Returns the hex
/// digest. Used to derive the canonical content-addressed filename.
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
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(hex)
}

/// Strip path separators and control characters from a user-supplied
/// filename so it cannot be used to escape into other directories. The
/// sanitized value is only used for display + the `Content-Disposition`
/// header; the actual storage path is content-addressed and never echoes
/// user input.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .filter(|c| !c.is_control())
        .take(255)
        .collect()
}

/// Escape a filename for the `Content-Disposition` `filename="..."` form.
/// Replaces backslashes and double-quotes; non-ASCII is preserved (modern
/// browsers handle UTF-8 in the unquoted "filename*" form, but the simple
/// quoted form is good enough for inline display).
fn sanitize_disposition_filename(s: &str) -> String {
    s.replace(['\\', '"'], "_")
}
