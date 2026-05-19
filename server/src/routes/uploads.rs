use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::uploads::pipeline::{self, PipelineError, ProcessedImage};

use crate::auth::{AuthUser, OptionalUser};
use crate::db;
use crate::db::uploads::WaveformBlob;
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

/// When the client flags an upload as a voice recording (`kind=voice`), the
/// sniffed type is the *container* the browser's `MediaRecorder` produced -
/// which `infer` reports as a `video/*` or generic-ogg type even for
/// audio-only data (Chrome -> webm, Firefox -> ogg, Safari -> mp4). Map the
/// recognised audio containers to a canonical `audio/*` MIME + storage
/// extension. Anything else returns `None` and the caller rejects with 415.
/// Magic-byte sniffing still ran first, so this only ever sees real media
/// containers - never an arbitrary uploaded blob.
fn allowed_voice_mime(sniffed: &str) -> Option<(&'static str, &'static str)> {
    match sniffed {
        "video/webm" | "audio/webm" => Some(("audio/webm", "webm")),
        "application/ogg" | "audio/ogg" | "video/ogg" => Some(("audio/ogg", "ogg")),
        "video/mp4" | "audio/mp4" | "audio/m4a" => Some(("audio/mp4", "m4a")),
        "audio/mpeg" => Some(("audio/mpeg", "mp3")),
        _ => None,
    }
}

/// Validate and normalize a client-supplied waveform blob. The wire format is
/// a JSON object `{ "d": duration_seconds, "p": [peaks] }`. Caps the peak
/// count, clamps every peak into 0..1, and drops a non-positive or non-finite
/// duration - so a hostile or buggy client cannot store an oversized or
/// out-of-range blob. Returns the re-serialized canonical JSON, or `None` if
/// the input is unusable, in which case the voice message simply renders
/// without a waveform.
fn sanitize_waveform(raw: &str) -> Option<String> {
    const MAX_PEAKS: usize = 256;
    let blob: WaveformBlob = serde_json::from_str(raw).ok()?;
    if blob.p.is_empty() {
        return None;
    }
    let peaks: Vec<f32> = blob
        .p
        .into_iter()
        .take(MAX_PEAKS)
        .map(|p| {
            if p.is_finite() {
                p.clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect();
    let duration = blob.d.filter(|d| d.is_finite() && *d > 0.0);
    serde_json::to_string(&WaveformBlob {
        d: duration,
        p: peaks,
    })
    .ok()
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

    // Voice recordings reuse this endpoint: the client sends `kind=voice` and
    // a `waveform` JSON array as text fields *before* the `file` field, so
    // both are populated by the time the file field is streamed below.
    let mut upload_kind: Option<String> = None;
    let mut waveform_json: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        match field.name() {
            Some("kind") => {
                upload_kind = field.text().await.ok();
                continue;
            }
            Some("waveform") => {
                waveform_json = field.text().await.ok().filter(|s| !s.is_empty());
                continue;
            }
            Some("file") => {}
            _ => continue,
        }
        let is_voice = upload_kind.as_deref() == Some("voice");
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

        // LC-93: per-user quota check. We use the post-stream `total` as
        // the upload's size estimate; the image re-encode below may
        // shrink it slightly but never grows the recognized formats,
        // so an under-cap upload will not be falsely rejected here. Run
        // before the (expensive) re-encode so a rejected upload does not
        // burn CPU on a file we are about to discard.
        if let Some(quota) = db::quota::get_user_quota(&state.chat, &user.id).await? {
            let used = db::quota::sum_user_usage(&state.chat, &user.id).await?;
            if used.saturating_add(total) > quota {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Ok((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!(
                        "upload would exceed your {quota}-byte storage quota \
                         (using {used}, this upload is {total})"
                    ),
                )
                    .into_response());
            }
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
        // For voice uploads the stored MIME is canonicalized from the sniffed
        // container; for everything else it stays exactly as sniffed.
        let (ext, canonical_mime): (&'static str, String) = if is_voice {
            match allowed_voice_mime(&mime_type) {
                Some((m, e)) => (e, m.to_string()),
                None => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Ok((
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        format!("unsupported voice format: {mime_type}"),
                    )
                        .into_response());
                }
            }
        } else {
            match allowed_ext_for_mime(&mime_type) {
                Some(e) => (e, mime_type.clone()),
                None => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Ok((
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        format!("unsupported file type: {mime_type}"),
                    )
                        .into_response());
                }
            }
        };

        let (storage_name, stored_size, stored_mime) = if mime_type.starts_with("image/") {
            // Acquire a permit. Decode + re-encode is memory-hungry; THUMBNAIL_CONCURRENCY
            // caps how many run at once. The permit drops at the end of this branch,
            // before the (much cheaper) DB insert.
            let _permit = crate::uploads::thumbnail_semaphore()
                .acquire()
                .await
                .expect("thumbnail semaphore never closed");

            // Pipeline is CPU- and sync-IO-bound; run on the blocking pool so the
            // async runtime threads stay free for other requests.
            let tmp_for_blocking = tmp_path.clone();
            let mime_for_blocking = mime_type.clone();
            let processed_result = tokio::task::spawn_blocking(move || {
                pipeline::process_image(&tmp_for_blocking, &mime_for_blocking)
            })
            .await
            .map_err(|e| AppError::Internal(format!("image pipeline join: {e}")))?;

            let processed = match processed_result {
                Ok(p) => p,
                Err(PipelineError::Decode(e)) => {
                    tracing::warn!(error = %e, "image decode failed");
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Ok(
                        (StatusCode::BAD_REQUEST, "image could not be decoded").into_response()
                    );
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Err(AppError::Internal(format!("image pipeline: {e}")));
                }
            };

            let ProcessedImage {
                original_bytes,
                preview_bytes,
                mime: stripped_mime,
            } = processed;

            // sha256 of the STRIPPED bytes (not the temp file). Dedup key reflects the
            // cleaned form so two users uploading the same photo with different EXIF
            // metadata dedup correctly.
            let hex = sha256_bytes(&original_bytes);
            let storage_name = format!("{hex}.{ext}");
            let final_path = uploads_root.join(&storage_name);
            let preview_name = crate::uploads::preview_storage_name(&storage_name);
            let preview_path = uploads_root.join(&preview_name);

            if tokio::fs::metadata(&final_path).await.is_ok() {
                // Dedup hit. Heal a missing preview if a prior upload's preview write
                // failed (disk full at the time, etc.). Do NOT rewrite the original.
                if tokio::fs::metadata(&preview_path).await.is_err() {
                    if let Err(e) =
                        crate::uploads::write_atomic(&preview_path, &preview_bytes).await
                    {
                        tracing::warn!(
                            error = %e,
                            path = %preview_path.display(),
                            "preview heal failed",
                        );
                    }
                }
            } else {
                crate::uploads::write_atomic(&final_path, &original_bytes)
                    .await
                    .map_err(|e| AppError::Internal(format!("write original: {e}")))?;
                // Preview is best-effort: the original is committed and the serve route
                // falls back to it if the preview is missing.
                if let Err(e) = crate::uploads::write_atomic(&preview_path, &preview_bytes).await {
                    tracing::warn!(
                        error = %e,
                        path = %preview_path.display(),
                        "preview write failed (original committed)",
                    );
                }
            }
            let _ = tokio::fs::remove_file(&tmp_path).await;

            (storage_name, original_bytes.len() as i64, stripped_mime)
        } else {
            // Non-image branch (PDF + voice recordings). Hash the temp file
            // and rename into content-addressed storage; no re-encoding.
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
            (storage_name, total, canonical_mime.clone())
        };

        // Waveform is only meaningful for voice uploads; ignore the field on
        // any other upload even if a client sends it.
        let waveform = if is_voice {
            waveform_json.as_deref().and_then(sanitize_waveform)
        } else {
            None
        };

        let id = db::uploads::insert_upload(
            &state.chat,
            &user.id,
            &original_filename,
            &stored_mime,
            stored_size,
            &storage_name,
            waveform.as_deref(),
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
    Query(params): Query<HashMap<String, String>>,
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

    let want_preview = params.get("size").map(|s| s == "preview").unwrap_or(false);

    // Resolve which file to serve. The preview branch derives its MIME from
    // the preview path via `mime_for_storage_path`, NOT from `row.mime_type`,
    // so a future format where preview-mime ≠ original-mime cannot accidentally
    // serve the wrong Content-Type. If the preview is missing on disk (write
    // failed earlier, or the row predates Phase 23), fall through to the
    // original.
    let (path, content_type, content_length) =
        if want_preview && row.mime_type.starts_with("image/") {
            let preview_name = crate::uploads::preview_storage_name(&row.storage_path);
            let preview_path = db::uploads_dir().join(&preview_name);
            match tokio::fs::metadata(&preview_path).await {
                Ok(meta) => {
                    let mime = crate::uploads::mime_for_storage_path(&preview_name)
                        .map(String::from)
                        .unwrap_or_else(|| {
                            tracing::warn!(
                                path = %preview_path.display(),
                                "preview mime not in mapping; falling back to row mime",
                            );
                            row.mime_type.clone()
                        });
                    (preview_path, mime, meta.len())
                }
                Err(_) => (
                    db::uploads_dir().join(&row.storage_path),
                    row.mime_type.clone(),
                    row.size_bytes as u64,
                ),
            }
        } else {
            (
                db::uploads_dir().join(&row.storage_path),
                row.mime_type.clone(),
                row.size_bytes as u64,
            )
        };

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
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CONTENT_LENGTH, content_length.to_string()),
            (header::CACHE_CONTROL, "private, max-age=86400".to_string()),
        ],
        body,
    )
        .into_response())
}

/// Sha256 an in-memory byte slice. Used for image uploads where the canonical
/// filename derives from the STRIPPED re-encoded bytes, not the temp file.
fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
