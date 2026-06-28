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
    let enclave_role = if let Some(eid) = super::enclave_for_room(&state, room_id).await? {
        db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role)
    } else {
        None
    };
    let can_manage_overrides = crate::perms::room_can_manage_overrides(enclave_role, &user.role);

    let fragment = RoomHeaderFragment {
        room: &room,
        mute_mode: mode.as_str(),
        can_manage_overrides,
        llm_available: state.llm_available(),
        llm_teaser: !state.llm_available() && user.role == "admin",
    };
    html(&fragment)
}
