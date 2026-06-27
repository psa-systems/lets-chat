use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::mentions::{BroadcastCountFragment, MentionPopoverFragment, MentionSuggestion};
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
    // LC-476: only offer @here/@channel when the room's broadcast policy lets
    // this viewer use them; otherwise the send path would silently drop the
    // tokens, so suggesting them would be misleading.
    let can_broadcast = room.room_type != "dm"
        && super::room::broadcast_allowed_for_user(&state, room_id, &viewer).await?;
    if room.room_type != "dm" {
        if can_broadcast && "here".starts_with(&q_lower) {
            results.push(MentionSuggestion::broadcast(
                "here",
                "Notify online members",
            ));
        }
        if can_broadcast && "channel".starts_with(&q_lower) {
            results.push(MentionSuggestion::broadcast(
                "channel",
                "Notify the entire room",
            ));
        }
        // User groups live per-enclave. Rooms outside an enclave (DMs above,
        // legacy enclave-less public rooms) cannot match a group, so the
        // resolver in routes/room.rs treats them as plain text; skip the
        // lookup here too.
        if let Some(enclave_id) = super::enclave_for_room(&state, room_id).await? {
            let groups = db::user_groups::list_for_enclave(&state.chat, enclave_id).await?;
            for g in groups {
                if results.len() >= MAX {
                    break;
                }
                if !trimmed.is_empty() && !g.name.to_ascii_lowercase().contains(&q_lower) {
                    continue;
                }
                let count = db::user_groups::list_member_ids(&state.chat, g.id)
                    .await?
                    .len() as i64;
                results.push(MentionSuggestion::group(g.name, count));
            }
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
        // Resolve presence the same way every other avatar surface does, so
        // the mention row's badge (rendered via partials/avatar.html) matches
        // user search / the sidebar (LC-169). Candidates exclude the viewer,
        // so effective_status (offline-when-disconnected) is always correct.
        let status = super::effective_status(&state, &rec.id, &rec.status);
        results.push(MentionSuggestion::user(
            rec.id,
            rec.username,
            rec.display_name,
            rec.avatar_ext,
            status,
            rec.custom_status,
        ));
    }

    let frag = MentionPopoverFragment { results: &results };
    html(&frag)
}

#[derive(Deserialize)]
pub struct BroadcastCountQuery {
    pub token: String,
}

/// GET /api/rooms/:room_id/broadcast-count?token=here|channel
///
/// Live "this will notify N people" probe fired by the composer when the
/// active token at the cursor is a broadcast token. Returns an HTML
/// fragment (not JSON) so the composer can swap it directly into the
/// `#lc-broadcast-count` slot via `htmx.ajax`. Token validation rejects
/// anything other than `here` / `channel` with 400; DM rooms also return
/// 400 because broadcast resolution is a no-op there.
///
/// Reuses the same resolver helpers that `post_message` uses, so the count
/// shown to the sender equals the number of mention rows that would be
/// written. Cost: one bulk auth query + one hub presence pass for `@here`,
/// or just the auth bulk for `@channel`. Both clear the scale budget
/// comfortably at the v1 scale ceiling (200-person rooms).
pub async fn get_broadcast_count(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    axum::extract::Path(room_id): axum::extract::Path<i64>,
    Query(BroadcastCountQuery { token }): Query<BroadcastCountQuery>,
) -> Result<Html, AppError> {
    let token_lower = token.to_ascii_lowercase();
    if token_lower != "here" && token_lower != "channel" {
        return Err(AppError::BadRequest(
            "token must be 'here' or 'channel'".into(),
        ));
    }

    let is_admin = viewer.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &viewer.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "dm" {
        return Err(AppError::BadRequest(
            "broadcast tokens are not valid in DM rooms".into(),
        ));
    }

    // LC-476: when the viewer isn't allowed to broadcast here, the send path
    // would drop the token, so the honest preview is zero (the fragment renders
    // nothing for count == 0).
    let count = if !super::room::broadcast_allowed_for_user(&state, room_id, &viewer).await? {
        0
    } else if token_lower == "here" {
        super::room::resolve_here_targets(&state, &room, &viewer.id)
            .await?
            .len() as i64
    } else {
        super::room::resolve_channel_targets(&state, &room, &viewer.id)
            .await?
            .len() as i64
    };

    let frag = BroadcastCountFragment {
        count,
        room_name: &room.name,
    };
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
