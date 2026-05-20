//! Branding logo routes + the CSS-var injection middleware (LC-96).

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::OptionalUser;
use crate::db;
use crate::db::branding::{Branding, Scope};
use crate::error::AppError;
use crate::state::AppState;

/// `GET /branding/logo` - public route. Serves the global branding
/// logo, or a 404 when no logo is set. Cache-busted via the
/// `?v=updated_at` query param the layout templates include.
pub async fn get_global_logo(State(state): State<AppState>) -> Result<Response, AppError> {
    serve_logo_for_scope(&state, Scope::Global).await
}

/// `GET /enclave/{id}/branding/logo` - membership-gated. Falls back
/// to the global logo when the enclave has no row of its own, so the
/// per-enclave URL always returns something sensible.
pub async fn get_enclave_logo(
    State(state): State<AppState>,
    OptionalUser(maybe_user): OptionalUser,
    Path(enclave_id): Path<i64>,
) -> Result<Response, AppError> {
    // Gate to authenticated users only; we don't want public access
    // to a per-enclave logo (operators may use it as a "we have this
    // tenant" signal).
    let Some(user) = maybe_user else {
        return Err(AppError::Unauthorized);
    };
    // Membership check: site admins always pass; everyone else needs
    // to be a member of the enclave. This mirrors the read access
    // policy used elsewhere for enclave-scoped data.
    let is_admin = user.role == "admin";
    if !is_admin {
        let member = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
        if member.is_none() {
            return Err(AppError::Forbidden);
        }
    }
    serve_logo_for_scope(&state, Scope::Enclave(enclave_id)).await
}

async fn serve_logo_for_scope(state: &AppState, scope: Scope) -> Result<Response, AppError> {
    let branding = db::branding::resolve(&state.chat, scope).await?;
    let Some(file_id) = branding.logo_upload_id else {
        return Err(AppError::NotFound);
    };
    let (row, _) = db::uploads::get_upload(&state.chat, file_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let path = db::uploads_dir().join(&row.storage_path);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, row.mime_type),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// Tower middleware: stamps the resolved branding's CSS variables
/// into every full-page text/html response so colors propagate
/// without any per-template field threading. The `<style>` block is
/// inserted right before the closing `</head>` tag.
///
/// Skipped entirely for:
/// - HTMX requests (`HX-Request: true`) - those responses are
///   fragments with no `<head>`, and they are the high-frequency
///   path (every message send). Skipping them avoids the buffer +
///   the per-request branding DB query.
/// - non-HTML or non-2xx responses.
/// - responses with no `</head>` (fragments / error bodies) - left
///   byte-for-byte unchanged.
pub async fn inject_branding_css(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    // Fast path: HTMX fragment requests never carry a <head>, so
    // there is nothing to inject. Bail before touching the DB or
    // buffering the body.
    let is_htmx = req
        .headers()
        .get("hx-request")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if is_htmx {
        return next.run(req).await;
    }

    // Resolve scope from the URL path BEFORE invoking the inner
    // service, so the scope reflects what the user navigated to (and
    // matches the templates that will reference per-enclave assets).
    let scope = match crate::db::branding::enclave_id_from_path(req.uri().path()) {
        Some(id) => Scope::Enclave(id),
        None => Scope::Global,
    };
    let branding = db::branding::resolve(&state.chat, scope)
        .await
        .unwrap_or_else(|_| Branding::defaults_for_global());

    let res = next.run(req).await;
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.starts_with("text/html"));
    if !is_html || !res.status().is_success() {
        return res;
    }

    let (parts, body) = res.into_parts();
    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            // The body stream errored or blew the (generous) cap;
            // it is gone either way - `to_bytes` consumed it. We
            // cannot serve a partial page, and returning the
            // original status with an empty body would mask the
            // failure as a successful blank page, so surface a 500.
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error",
            )
                .into_response();
        }
    };
    let original = String::from_utf8_lossy(&bytes);
    // Defense-in-depth: trust nothing from the DB on the read path.
    // The write paths validate colors, but a manual DB edit (or a
    // future un-validated writer) must not be able to break out of
    // the `<style>` block. Fall back to the safe defaults on any
    // invalid value.
    let primary = if crate::db::branding::is_valid_hex_color(&branding.primary_color) {
        branding.primary_color.as_str()
    } else {
        crate::db::branding::DEFAULT_PRIMARY
    };
    let accent = if crate::db::branding::is_valid_hex_color(&branding.accent_color) {
        branding.accent_color.as_str()
    } else {
        crate::db::branding::DEFAULT_ACCENT
    };
    let style = format!(
        "<style data-lc-brand>:root{{--brand-primary:{primary};--brand-accent:{accent}}}</style>"
    );
    let injected = if let Some(idx) = original.find("</head>") {
        let mut out = String::with_capacity(original.len() + style.len());
        out.push_str(&original[..idx]);
        out.push_str(&style);
        out.push_str(&original[idx..]);
        out
    } else {
        // Fragment / no <head>: leave as-is.
        original.into_owned()
    };
    let mut new_res = Response::from_parts(parts, Body::from(injected));
    // Length changed; let the runtime re-derive it.
    new_res.headers_mut().remove(header::CONTENT_LENGTH);
    new_res
}

/// Shared multipart parser for the admin + per-enclave branding
/// forms. Fields: `primary_color`, `accent_color`, `login_heading`,
/// `login_body` (text), `logo` (optional file). On parse error or
/// invalid input, returns `Ok(Err(message))` so the caller can
/// re-render its own page with the inline error.
pub async fn parse_branding_multipart(
    state: &AppState,
    actor_id: &str,
    mut multipart: axum::extract::Multipart,
) -> Result<Result<BrandingForm, String>, AppError> {
    let mut out = BrandingForm::default();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        match field.name() {
            Some("primary_color") => out.primary_color = field.text().await.ok(),
            Some("accent_color") => out.accent_color = field.text().await.ok(),
            Some("login_heading") => out.login_heading = field.text().await.ok(),
            Some("login_body") => out.login_body = field.text().await.ok(),
            Some("logo") => match persist_logo_field(state, actor_id, &mut field).await? {
                Ok(maybe_id) => out.new_logo_id = maybe_id,
                Err(msg) => return Ok(Err(msg)),
            },
            _ => {}
        }
    }
    Ok(Ok(out))
}

#[derive(Default, Debug)]
pub struct BrandingForm {
    pub primary_color: Option<String>,
    pub accent_color: Option<String>,
    pub login_heading: Option<String>,
    pub login_body: Option<String>,
    pub new_logo_id: Option<i64>,
}

async fn persist_logo_field(
    state: &AppState,
    actor_id: &str,
    field: &mut axum::extract::multipart::Field<'_>,
) -> Result<Result<Option<i64>, String>, AppError> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    let original_filename = field.file_name().unwrap_or("logo.bin").to_string();
    let tmp_dir = db::uploads_dir().join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir tmp: {e}")))?;
    let tmp_path = tmp_dir.join(format!("brand-{}", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| AppError::Internal(format!("create tmp: {e}")))?;
    let mut total: i64 = 0;
    let mut overflow = false;
    while let Some(chunk) = field.next().await {
        let chunk = chunk.map_err(|e| AppError::BadRequest(format!("multipart read: {e}")))?;
        total = total.saturating_add(chunk.len() as i64);
        if total > 1024 * 1024 {
            overflow = true;
            break;
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| AppError::Internal(format!("write tmp: {e}")))?;
    }
    file.flush().await.ok();
    drop(file);
    if total == 0 {
        // No file actually uploaded - the input was empty. Not an error.
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Ok(Ok(None));
    }
    if overflow {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Ok(Err("Logo file exceeds 1 MiB limit".to_string()));
    }
    let (mime_type, ext) = match infer::get_from_path(&tmp_path) {
        Ok(Some(k)) => match k.mime_type() {
            m @ "image/png" => (m.to_string(), "png"),
            m @ "image/jpeg" => (m.to_string(), "jpg"),
            m @ "image/webp" => (m.to_string(), "webp"),
            m @ "image/gif" => (m.to_string(), "gif"),
            m => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Ok(Err(format!("Unsupported logo type: {m}")));
            }
        },
        _ => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Ok(Err("Could not determine logo file type".to_string()));
        }
    };
    let hex = {
        use sha2::{Digest, Sha256};
        let bytes = tokio::fs::read(&tmp_path)
            .await
            .map_err(|e| AppError::Internal(format!("read tmp: {e}")))?;
        let mut h = Sha256::new();
        h.update(&bytes);
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let storage_name = format!("{hex}.{ext}");
    let final_path = db::uploads_dir().join(&storage_name);
    if tokio::fs::metadata(&final_path).await.is_err() {
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|e| AppError::Internal(format!("rename: {e}")))?;
    } else {
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
    let id = db::uploads::insert_upload(
        &state.chat,
        actor_id,
        &original_filename,
        &mime_type,
        total,
        &storage_name,
        None,
    )
    .await?;
    Ok(Ok(Some(id)))
}

/// Helper for the routes that need a path to a `data_dir` upload.
/// Kept here so the route layer doesn't import `db::` for path
/// construction.
#[allow(dead_code)]
pub fn upload_path(storage_path: &str) -> PathBuf {
    db::uploads_dir().join(storage_path)
}
