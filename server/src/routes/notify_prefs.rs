use axum::extract::{Path, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::notify_prefs::RoomHeaderFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Deserialize)]
pub struct NotifyPrefsForm {
    pub mute_mode: String,
}

/// POST /room/:id/notify-prefs
///
/// Persist the viewer's mute mode for `room_id` and return the swapped
/// `#lc-room-header` fragment so the caller's tab updates inline. Other
/// open tabs of the same user receive a `RoomNotifyPrefsChanged` event over
/// WS and re-render their sidebar (greyed-name + badge visibility flips).
pub async fn post_notify_prefs(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<NotifyPrefsForm>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // DM rooms can't have notification preferences set via this surface
    // (per-DM mute is its own future phase). The room/page.html template
    // is the only place that renders the dropdown, but harden the handler
    // anyway against hand-crafted POSTs.
    if room.room_type == "dm" {
        return Err(AppError::BadRequest("DM mute is not supported".into()));
    }

    let mode = db::notifications::MuteMode::parse_str(&form.mute_mode)
        .ok_or_else(|| AppError::BadRequest(format!("invalid mute_mode: {}", form.mute_mode)))?;
    db::notifications::set_room_mute_mode(&state.chat, &user.id, room_id, mode).await?;

    // Fan out a per-user event so other tabs of the same user re-render
    // their sidebar. The requesting tab updates inline via the response
    // body below.
    let event = ChatEvent::RoomNotifyPrefsChanged {
        user_id: user.id.clone(),
        room_id,
        mute_mode: mode.as_str().to_string(),
    };
    state.hub.broadcast_to_user(&user.id, &event);

    // LC-84: re-resolve the override-manage flag so the rendered header
    // keeps (or hides) the "Moderators" link consistently with the
    // initial page render.
    let current_enclave = super::enclave_for_room(&state, room_id).await?;
    let enclave_role = if let Some(eid) = current_enclave {
        db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role)
    } else {
        None
    };
    let can_manage_overrides = crate::perms::room_can_manage_overrides(enclave_role, &user.role);

    // LC-553: recompute the header member stack (mirrors RoomPage) so it
    // survives this notify-prefs header swap. Public rooms use enclave members.
    let member_ids: Vec<String> = if room.room_type != "private" {
        if let Some(eid) = current_enclave {
            db::enclave::list_members(&state.chat, eid)
                .await?
                .into_iter()
                .map(|m| m.user_id)
                .collect()
        } else {
            db::chat::list_room_member_ids(&state.chat, room_id).await?
        }
    } else {
        db::chat::list_room_member_ids(&state.chat, room_id).await?
    };

    let is_starred = db::starred_rooms::is_starred(&state.auth, &user.id, room.id).await?;
    // LC-679/LC-702: audience-scoped AI gate for the room-header AI affordances.
    let ai_flag_on = super::ai_gate::flag_on(&state).await;
    let ai_llm = ai_flag_on
        && state.llm_available()
        && super::ai_gate::allowed_in_room(&state, room.id, &user).await?;
    let fragment = RoomHeaderFragment {
        room: &room,
        mute_mode: mode.as_str(),
        is_starred,
        can_manage_overrides,
        sidebar_current_enclave: current_enclave,
        llm_available: ai_llm,
        llm_teaser: !state.llm_available() && user.role == "admin",
        member_count: member_ids.len(),
        header_members: member_ids.iter().take(5).cloned().collect(),
    };
    html(&fragment)
}
