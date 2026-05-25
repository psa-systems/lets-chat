//! LC-77 email-attachment upload pipeline.
//!
//! Takes the bytes-in-memory `RawAttachment` produced by `parse.rs`,
//! writes them to a temp file, magic-byte sniffs via `infer::get_from_path`
//! (NOT the sender's Content-Type, which is spoofable), runs them
//! through the standard upload allowlist + image re-encode (EXIF strip),
//! and inserts a row in `file_uploads` via `db::uploads::insert_upload`
//! with `uploader_id=""` (the same sentinel webhook posts use for
//! `messages.user_id`).
//!
//! Returns the new upload id on success; the caller links it to the
//! freshly-inserted message via `db::uploads::link_upload_to_message`.
//!
//! All drops at this layer are NON-fatal to the parent message: the
//! body still posts, the bad attachment is logged INFO. This matches
//! the brainstorm decision "drop the over-cap attachment, log,
//! continue with other parts."

use std::path::PathBuf;
use uuid::Uuid;

use crate::db;
use crate::state::AppState;
use crate::uploads::{pipeline, sha256_bytes, sha256_file, write_atomic};

use super::parse::RawAttachment;

/// Default upload cap mirror of `routes::uploads::DEFAULT_MAX_UPLOAD_BYTES`.
/// Kept separate so a future divergence (e.g. email-attachment-specific
/// override) doesn't drag the web upload along. Today both are 10 MiB.
const DEFAULT_MAX_UPLOAD_BYTES: i64 = 10 * 1024 * 1024;

/// Read `settings.max_upload_bytes`, falling back to the default. Same
/// resolver shape as the web upload path.
async fn max_upload_bytes(state: &AppState) -> i64 {
    db::settings::get_setting(&state.settings, "max_upload_bytes")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_BYTES)
}

/// Allowlist of MIME types email attachments may store. Matches the
/// `allowed_ext_for_mime` allowlist in `routes::uploads`. Email ingress
/// intentionally does NOT support voice formats (no MediaRecorder origin
/// for an email attachment); add the voice MIMEs later if a real use
/// case appears.
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

/// Reason an attachment was rejected by the pipeline. Each is logged at
/// INFO (the parent message still posts) with the attachment's
/// pipeline-supplied filename in the log record.
#[derive(Debug, Clone)]
pub enum AttachmentDrop {
    OverSize { size: i64, cap: i64 },
    DisallowedMime { sniffed_mime: String },
    SniffFailed,
    ImagePipeline { detail: String },
    Io { detail: String },
    Db { detail: String },
}

impl AttachmentDrop {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OverSize { .. } => "over_size",
            Self::DisallowedMime { .. } => "disallowed_mime",
            Self::SniffFailed => "sniff_failed",
            Self::ImagePipeline { .. } => "image_pipeline",
            Self::Io { .. } => "io",
            Self::Db { .. } => "db",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::OverSize { size, cap } => format!("size {size} over cap {cap}"),
            Self::DisallowedMime { sniffed_mime } => {
                format!("sniffed MIME {sniffed_mime} not in allowlist")
            }
            Self::SniffFailed => "infer::get_from_path returned None".to_string(),
            Self::ImagePipeline { detail } => detail.clone(),
            Self::Io { detail } => detail.clone(),
            Self::Db { detail } => detail.clone(),
        }
    }
}

/// Process one email attachment end-to-end through the upload pipeline.
/// Returns the new `file_uploads.id` on success; the caller links it to
/// a message via `db::uploads::link_upload_to_message`.
///
/// On failure, the temp file is cleaned up best-effort and the returned
/// `AttachmentDrop` carries the operator-readable reason + detail.
pub async fn process_attachment(
    state: &AppState,
    raw: &RawAttachment,
) -> Result<i64, AttachmentDrop> {
    let cap = max_upload_bytes(state).await;
    let size = raw.bytes.len() as i64;
    if size > cap {
        return Err(AttachmentDrop::OverSize { size, cap });
    }

    // Stream bytes to a temp file so infer + the image pipeline can both
    // work against a path (matches the web upload path's shape).
    let uploads_root = db::uploads_dir();
    let tmp_dir = uploads_root.join(".tmp");
    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        return Err(AttachmentDrop::Io {
            detail: format!("mkdir {}: {e}", tmp_dir.display()),
        });
    }
    let tmp_path: PathBuf = tmp_dir.join(format!("ei-{}", Uuid::new_v4()));
    if let Err(e) = tokio::fs::write(&tmp_path, &raw.bytes).await {
        return Err(AttachmentDrop::Io {
            detail: format!("write temp {}: {e}", tmp_path.display()),
        });
    }

    // Magic-byte sniff. The sender's Content-Type is NOT trusted.
    let sniffed = match infer::get_from_path(&tmp_path) {
        Ok(Some(k)) => k,
        Ok(None) | Err(_) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(AttachmentDrop::SniffFailed);
        }
    };
    let sniffed_mime = sniffed.mime_type().to_string();
    let Some(ext) = allowed_ext_for_mime(&sniffed_mime) else {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(AttachmentDrop::DisallowedMime { sniffed_mime });
    };

    let (storage_name, stored_size, stored_mime) = if sniffed_mime.starts_with("image/") {
        // Cap concurrent image decodes the same way the web path does.
        let _permit = crate::uploads::thumbnail_semaphore()
            .acquire()
            .await
            .expect("thumbnail semaphore never closed");
        let tmp_for_blocking = tmp_path.clone();
        let mime_for_blocking = sniffed_mime.clone();
        let processed = tokio::task::spawn_blocking(move || {
            pipeline::process_image(&tmp_for_blocking, &mime_for_blocking)
        })
        .await
        .map_err(|e| AttachmentDrop::ImagePipeline {
            detail: format!("join: {e}"),
        })?
        .map_err(|e| AttachmentDrop::ImagePipeline {
            detail: format!("{e}"),
        })?;
        let pipeline::ProcessedImage {
            original_bytes,
            preview_bytes,
            mime: stripped_mime,
        } = processed;
        // Content-addressed name on the STRIPPED bytes, matching the web
        // path. Dedup keys reflect the cleaned form so the same photo
        // from two senders (with different EXIF) maps to one stored file.
        let hex = sha256_bytes(&original_bytes);
        let storage_name = format!("{hex}.{ext}");
        let final_path = uploads_root.join(&storage_name);
        let preview_name = crate::uploads::preview_storage_name(&storage_name);
        let preview_path = uploads_root.join(&preview_name);
        if tokio::fs::metadata(&final_path).await.is_err() {
            if let Err(e) = write_atomic(&final_path, &original_bytes).await {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(AttachmentDrop::Io {
                    detail: format!("write original: {e}"),
                });
            }
            // Preview is best-effort, like the web path: the original is
            // already committed and the serve route falls back to it if
            // the preview is missing.
            if let Err(e) = write_atomic(&preview_path, &preview_bytes).await {
                tracing::warn!(
                    target: "email_ingress",
                    error = %e,
                    path = %preview_path.display(),
                    "attachment preview write failed (original committed)",
                );
            }
        } else if tokio::fs::metadata(&preview_path).await.is_err() {
            // Dedup hit but preview missing (a prior upload's preview
            // write failed). Heal it; do NOT rewrite the original.
            if let Err(e) = write_atomic(&preview_path, &preview_bytes).await {
                tracing::warn!(
                    target: "email_ingress",
                    error = %e,
                    path = %preview_path.display(),
                    "attachment preview heal failed",
                );
            }
        }
        let _ = tokio::fs::remove_file(&tmp_path).await;
        (storage_name, original_bytes.len() as i64, stripped_mime)
    } else {
        // Non-image (PDF). Hash the temp file and rename into content-
        // addressed storage; no re-encoding. Mirrors the web path.
        let hex = sha256_file(&tmp_path)
            .await
            .map_err(|e| AttachmentDrop::Io {
                detail: format!("hash temp: {e}"),
            })?;
        let storage_name = format!("{hex}.{ext}");
        let final_path = uploads_root.join(&storage_name);
        match tokio::fs::metadata(&final_path).await {
            Ok(_) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
            }
            Err(_) => {
                if let Err(e) = tokio::fs::rename(&tmp_path, &final_path).await {
                    return Err(AttachmentDrop::Io {
                        detail: format!("rename temp: {e}"),
                    });
                }
            }
        }
        (storage_name, size, sniffed_mime)
    };

    // Insert the upload row with the synthetic-actor sentinel
    // (uploader_id=""), mirroring messages.user_id='' for webhook posts.
    // No waveform field for email; voice ingress is not a v1 use case.
    let upload_id = db::uploads::insert_upload(
        &state.chat,
        "",
        &raw.filename,
        &stored_mime,
        stored_size,
        &storage_name,
        None,
    )
    .await
    .map_err(|e| AttachmentDrop::Db {
        detail: format!("insert_upload: {e}"),
    })?;
    Ok(upload_id)
}
