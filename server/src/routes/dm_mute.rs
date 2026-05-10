use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::dm_header::DmHeaderFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Deserialize, Default)]
pub struct DmMuteForm {
    /// Standard HTML checkbox semantics: present (any value) when checked,
    /// absent when unchecked. Serde's `Option<String>` covers both.
    pub muted: Option<String>,
}

/// POST /dm/:peer_id/mute
///
/// Toggle the viewer's mute setting for their DM with `peer_id`. Returns
/// the swapped `#lc-dm-header` fragment so the requesting tab updates
/// inline. Other open tabs of the same user receive a `DmMuteChanged`
/// event over WS and re-render their sidebar.
///
/// Block interaction: blocked DMs render as `WelcomePage` instead of the
/// DM page (`routes/dm.rs::get_dm`), so the toggle is unreachable from
/// the UI. The handler does not duplicate the block check - mute is a
/// private per-user setting and muting an already-existing DM is harmless
/// even when the conversation is gated.
pub async fn post_dm_mute(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(peer_id): Path<String>,
    Form(form): Form<DmMuteForm>,
) -> Result<Html, AppError> {
    if peer_id == user.id {
        return Err(AppError::BadRequest(
            "cannot mute a DM with yourself".into(),
        ));
    }

    let peer_record = db::auth::find_user_by_id(&state.auth, &peer_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut peer: User = peer_record.into();
    peer.status = super::effective_status(&state, &peer.id, &peer.status);

    let room = db::chat::find_dm_room(&state.chat, &user.id, &peer.id)
        .await?
        .ok_or(AppError::NotFound)?;

    if !db::chat::is_room_member(&state.chat, room.id, &user.id).await? {
        return Err(AppError::Forbidden);
    }

    let muted = form.muted.is_some();
    db::notifications::set_dm_mute(&state.chat, &user.id, room.id, muted).await?;

    let event = ChatEvent::DmMuteChanged {
        dm_room_id: room.id,
        peer_user_id: peer.id.clone(),
        muted,
    };
    state.hub.broadcast_to_user(&user.id, &event);

    let fragment = DmHeaderFragment {
        peer: &peer,
        room: &room,
        mute_mode: if muted { "all" } else { "none" },
    };
    html(&fragment)
}
