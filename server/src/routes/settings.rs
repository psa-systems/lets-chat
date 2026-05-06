use axum::extract::{Multipart, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::settings::{BlockedListPage, BlockedUserView, UserSettingsPage};
use crate::views::{html, Html};

const MAX_AVATAR_BYTES: usize = 1024 * 1024;
const MAX_DISPLAY_NAME_CHARS: usize = 64;
const MAX_BIO_CHARS: usize = 500;

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(default)]
    pub read_receipts_enabled: Option<String>,
    #[serde(default)]
    pub is_profile_public: Option<String>,
    #[serde(default)]
    pub notify_browser_enabled: Option<String>,
    #[serde(default)]
    pub notify_sound_enabled: Option<String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let page = UserSettingsPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        saved: false,
    };
    html(&page)
}

pub async fn post_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<SettingsForm>,
) -> Result<Response, AppError> {
    let enabled = form.read_receipts_enabled.is_some();
    db::auth::set_read_receipts_enabled(&state.auth, &user.id, enabled).await?;
    let is_public = form.is_profile_public.is_some();
    db::auth::set_profile_public(&state.auth, &user.id, is_public).await?;
    let browser = form.notify_browser_enabled.is_some();
    let sound = form.notify_sound_enabled.is_some();
    db::auth::set_notification_prefs(&state.auth, &user.id, browser, sound).await?;
    Ok(Redirect::to("/settings").into_response())
}

pub async fn post_profile(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut display_name: Option<String> = None;
    let mut bio: Option<String> = None;
    let mut avatar_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        match field.name().unwrap_or("") {
            "display_name" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("display_name: {e}")))?;
                let trimmed = v.trim();
                if trimmed.chars().count() > MAX_DISPLAY_NAME_CHARS {
                    return Err(AppError::BadRequest(format!(
                        "display name exceeds {MAX_DISPLAY_NAME_CHARS} characters"
                    )));
                }
                display_name = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            "bio" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("bio: {e}")))?;
                let trimmed = v.trim();
                if trimmed.chars().count() > MAX_BIO_CHARS {
                    return Err(AppError::BadRequest(format!(
                        "bio exceeds {MAX_BIO_CHARS} characters"
                    )));
                }
                bio = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            "avatar" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("avatar: {e}")))?;
                if bytes.is_empty() {
                    continue;
                }
                if bytes.len() > MAX_AVATAR_BYTES {
                    return Err(AppError::BadRequest("avatar exceeds 1 MiB".to_string()));
                }
                avatar_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    db::auth::update_user_profile(
        &state.auth,
        &user.id,
        display_name.as_deref(),
        bio.as_deref(),
    )
    .await?;

    if let Some(bytes) = avatar_bytes {
        let new_ext = sniff_image_ext(&bytes)
            .ok_or_else(|| AppError::BadRequest("avatar must be PNG, JPEG, or WebP".to_string()))?;
        let dir = db::avatars_dir();
        let final_path = dir.join(format!("{}.{}", user.id, new_ext));
        let tmp_path = dir.join(format!("{}.{}.tmp", user.id, new_ext));
        tokio::fs::write(&tmp_path, &bytes)
            .await
            .map_err(|e| AppError::Internal(format!("write avatar: {e}")))?;
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|e| AppError::Internal(format!("rename avatar: {e}")))?;
        if let Some(prev) = user.avatar_ext.as_deref() {
            if prev != new_ext {
                let prev_path = dir.join(format!("{}.{}", user.id, prev));
                let _ = tokio::fs::remove_file(prev_path).await;
            }
        }
        db::auth::set_user_avatar_ext(&state.auth, &user.id, Some(new_ext)).await?;
    }

    Ok(Redirect::to("/settings").into_response())
}

pub async fn get_blocked_list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    render_blocked_list(&state, &user, None, "").await
}

#[derive(Deserialize)]
pub struct BlockByUsernameForm {
    pub username: String,
}

/// POST /settings/blocked - block a user looked up by username. Lets the
/// caller block users with private profiles, who would not appear in the
/// public people-search results. Re-renders the page with an inline error
/// when the username is blank, missing, or refers to the caller themselves.
pub async fn post_block_by_username(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<BlockByUsernameForm>,
) -> Result<Response, AppError> {
    let raw = form.username.trim();
    let trimmed = raw.strip_prefix('@').unwrap_or(raw);
    if trimmed.is_empty() {
        return render_blocked_list(&state, &user, Some("Enter a username."), "")
            .await
            .map(|h| h.into_response());
    }
    let target = db::auth::find_user_by_username(&state.auth, trimmed).await?;
    let unknown_msg = format!("No user found with username @{trimmed}.");
    let Some(target) = target else {
        return render_blocked_list(&state, &user, Some(&unknown_msg), trimmed)
            .await
            .map(|h| h.into_response());
    };
    if target.id == user.id {
        return render_blocked_list(&state, &user, Some("You can't block yourself."), "")
            .await
            .map(|h| h.into_response());
    }
    // Privacy gate: only allow blocking by username when the target shares an
    // enclave with the caller. Public profiles bypass this check because
    // they're already discoverable through people-search. Without this gate,
    // the username form leaks the existence of every account regardless of
    // its privacy setting and lets an abuser pre-emptively block a private
    // user purely from a guessed username. The error message intentionally
    // matches the "user not found" branch so the response cannot
    // distinguish the two cases.
    if !target.is_profile_public
        && !db::enclave::users_share_enclave(&state.chat, &user.id, &target.id).await?
    {
        return render_blocked_list(&state, &user, Some(&unknown_msg), trimmed)
            .await
            .map(|h| h.into_response());
    }
    db::auth::block_user(&state.auth, &user.id, &target.id).await?;
    Ok(Redirect::to("/settings/blocked").into_response())
}

async fn render_blocked_list(
    state: &AppState,
    user: &crate::models::User,
    error: Option<&str>,
    form_username: &str,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(state, user, None).await?;
    let records = db::auth::list_blocked_users(&state.auth, &user.id).await?;
    let blocked: Vec<BlockedUserView> = records
        .into_iter()
        .map(|r| BlockedUserView {
            id: r.id,
            username: r.username,
            display_name: r.display_name,
            avatar_ext: r.avatar_ext,
        })
        .collect();
    let page = BlockedListPage {
        user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        blocked: &blocked,
        error,
        form_username,
    };
    html(&page)
}

pub async fn post_avatar_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    if let Some(ext) = user.avatar_ext.as_deref() {
        let path = db::avatars_dir().join(format!("{}.{}", user.id, ext));
        let _ = tokio::fs::remove_file(path).await;
    }
    db::auth::set_user_avatar_ext(&state.auth, &user.id, None).await?;
    Ok(Redirect::to("/settings").into_response())
}

fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some("png");
    }
    if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        return Some("jpg");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}
