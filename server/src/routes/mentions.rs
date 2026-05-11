use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::mentions::{MentionPopoverFragment, MentionSuggestion};
use crate::views::{html, Html};

const MAX: usize = 8;

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    pub room_id: i64,
    #[serde(default)]
    pub q: String,
}

/// GET /users/mentions?room_id=&q=
///
/// Returns a small `<ul>` of mention candidates the caller is allowed to use
/// in `room_id`. Broadcast tokens (`@here`, `@channel`) always come first
/// (Slack-style: higher-stakes tokens deserve the visibility, including when
/// the prefix matches both a broadcast token and a real username - `@h`
/// puts `@here` above `@harry`). Broadcast tokens are suppressed in DM
/// rooms because broadcast resolution is a no-op there.
///
/// Empty/whitespace `q` returns up to `MAX` candidates total. Always returns
/// 200 with an HTML body so the composer's `htmx.ajax(...)` can swap
/// directly into the popover slot.
pub async fn get_autocomplete(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Query(AutocompleteQuery { room_id, q }): Query<AutocompleteQuery>,
) -> Result<Html, AppError> {
    let trimmed = q.trim();

    let is_admin = viewer.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &viewer.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let q_lower = trimmed.to_ascii_lowercase();
    let mut results: Vec<MentionSuggestion> = Vec::with_capacity(MAX);

    // Broadcast suggestions go first. Non-DM rooms only - the resolver path
    // in routes/room.rs skips broadcast resolution for DMs, so showing the
    // tokens there would be a silent no-op confusing to the user.
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type != "dm" {
        if "here".starts_with(&q_lower) {
            results.push(MentionSuggestion::broadcast(
                "here",
                "Notify online members",
            ));
        }
        if "channel".starts_with(&q_lower) {
            results.push(MentionSuggestion::broadcast(
                "channel",
                "Notify the entire room",
            ));
        }
    }

    // User suggestions fill the remainder.
    let candidate_ids = candidate_ids(&state, room_id).await?;
    let viewer_id = viewer.id.clone();
    for id in candidate_ids {
        if id == viewer_id {
            continue;
        }
        if results.len() >= MAX {
            break;
        }
        let Some(rec) = db::auth::find_user_by_id(&state.auth, &id).await? else {
            continue;
        };
        if rec.is_banned {
            continue;
        }
        if !trimmed.is_empty() {
            let uname = rec.username.to_ascii_lowercase();
            let dname = rec
                .display_name
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if !uname.contains(&q_lower) && !dname.contains(&q_lower) {
                continue;
            }
        }
        results.push(MentionSuggestion::user(
            rec.id,
            rec.username,
            rec.display_name,
            rec.avatar_ext,
        ));
    }

    let frag = MentionPopoverFragment { results: &results };
    html(&frag)
}

/// Build the list of user IDs the caller is allowed to mention in `room_id`.
/// For private rooms and DMs that's the explicit `room_members`. For public
/// rooms it's the rooms's enclave members. If the room has no enclave
/// (legacy rows), fall back to all users.
async fn candidate_ids(state: &AppState, room_id: i64) -> Result<Vec<String>, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "private" || room.room_type == "dm" {
        return Ok(db::chat::list_room_member_ids(&state.chat, room_id).await?);
    }
    if let Some(enclave_id) = super::enclave_for_room(state, room_id).await? {
        let members = db::enclave::list_members(&state.chat, enclave_id).await?;
        return Ok(members.into_iter().map(|m| m.user_id).collect());
    }
    Ok(db::auth::list_user_ids(&state.auth).await?)
}
