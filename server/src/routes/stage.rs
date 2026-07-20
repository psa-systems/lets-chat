//! LC-494: stage control plane (speakers vs listeners + request-to-speak).
//!
//! This is the non-media half of the "Stage" feature: roles, the live roster,
//! and request-to-speak, all over the existing WebSocket. The actual audio
//! needs an SFU and lands in the LC-512 follow-up. State is ephemeral, in the
//! hub (`StageState`), like voice presence.
//!
//! Frames arrive in `routes::ws` (`ClientFrame::Stage*`); this module builds
//! the per-viewer roster panel rendered initially by `get_room` and live-swapped
//! on every `ChatEvent::StageChanged`.

use askama::Template;
use axum::extract::{Path, State};
use axum::Json;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::livekit::{self, LiveKitConfig};
use crate::models::User;
use crate::state::AppState;
use crate::views::stage::{StageMember, StagePanel};

/// A room is stage-eligible when stage mode is on and it is a group room (not a
/// DM). The caller still verifies access separately.
pub(crate) async fn stage_enabled(state: &AppState, room_id: i64) -> bool {
    match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) => {
            r.room_type != "dm"
                && db::chat::get_room_stage_enabled(&state.chat, room_id)
                    .await
                    .unwrap_or(false)
        }
        _ => false,
    }
}

/// True when `user` may host the stage (grant/revoke the floor): same set as
/// room management (enclave owner/admin or room moderator / site admin).
pub(crate) async fn is_host(state: &AppState, user: &User, room_id: i64) -> bool {
    super::room_rbac::require_can_manage(state, user, room_id)
        .await
        .is_ok()
}

/// Build the per-viewer stage panel for `room_id`. Renders even when the stage
/// is empty (so the viewer sees a "Join stage" button); the roster comes from
/// the hub and labels are resolved in one bulk auth lookup.
pub(crate) async fn build_panel(
    state: &AppState,
    viewer: &User,
    room_id: i64,
    oob: bool,
) -> StagePanel {
    let roster = state.hub.stage_roster(room_id).unwrap_or_default();
    let host = is_host(state, viewer, room_id).await;

    let ids: Vec<&str> = roster.participants.iter().map(|s| s.as_str()).collect();
    let labels = if ids.is_empty() {
        Default::default()
    } else {
        db::auth::display_names_for_ids(&state.auth, &ids)
            .await
            .unwrap_or_default()
    };
    let label_for = |uid: &str| -> String {
        labels
            .get(uid)
            .map(|(uname, dname)| match dname.as_deref() {
                Some(n) if !n.trim().is_empty() => n.to_string(),
                _ => uname.clone(),
            })
            .unwrap_or_else(|| uid.to_string())
    };

    let mut speakers: Vec<StageMember> = roster
        .speakers
        .iter()
        .map(|uid| StageMember {
            user_id: uid.clone(),
            label: label_for(uid),
            hand_raised: false,
        })
        .collect();
    speakers.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));

    let mut listeners: Vec<StageMember> = roster
        .participants
        .iter()
        .filter(|uid| !roster.speakers.contains(*uid))
        .map(|uid| StageMember {
            user_id: uid.clone(),
            label: label_for(uid),
            hand_raised: roster.hands.contains(uid),
        })
        .collect();
    // Raised hands first (so a host sees pending requests up top), then by name.
    listeners.sort_by(|a, b| {
        b.hand_raised
            .cmp(&a.hand_raised)
            .then(a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });

    let is_participant = roster.participants.contains(&viewer.id);
    let is_speaker = roster.speakers.contains(&viewer.id);
    let hand_raised = roster.hands.contains(&viewer.id);

    StagePanel {
        room_id,
        speakers,
        listeners,
        is_host: host,
        is_participant,
        is_speaker,
        hand_raised,
        livekit_available: livekit::available(),
        oob,
    }
}

/// Render the stage panel as an OOB swap for one viewer, or `None` when the
/// room is not stage-eligible (so no swap is emitted).
pub(crate) async fn render_panel(state: &AppState, viewer: &User, room_id: i64) -> Option<String> {
    if !stage_enabled(state, room_id).await {
        return None;
    }
    build_panel(state, viewer, room_id, true)
        .await
        .render()
        .ok()
}

#[derive(serde::Serialize)]
pub struct StageTokenResponse {
    /// Browser-facing LiveKit signaling URL.
    pub url: String,
    /// Short-lived LiveKit access token (JWT) scoped to this stage room.
    pub token: String,
    /// Whether this token grants mic publishing (true = speaker/host).
    pub can_publish: bool,
}

/// GET /room/{id}/stage/token  (LC-512)
///
/// Mint a LiveKit access token for the caller to join this room's stage. Gated
/// on: LiveKit configured, stage mode on, room access, and the caller currently
/// on the stage roster. Publish rights are derived live from the roster - a
/// speaker or host may publish; a listener may only subscribe - so the client
/// re-fetches this after a promote/demote to pick up its new permission.
pub async fn get_token(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Json<StageTokenResponse>, AppError> {
    let Some(cfg) = LiveKitConfig::from_env() else {
        return Err(AppError::BadRequest(
            "Stage audio is not configured on this server.".into(),
        ));
    };
    if !stage_enabled(&state, room_id).await {
        return Err(AppError::BadRequest(
            "Stage mode is off for this room.".into(),
        ));
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    // Must be on the stage roster to get audio (join via the control plane first).
    if !state.hub.stage_has_participant(room_id, &user.id) {
        return Err(AppError::BadRequest(
            "Join the stage before connecting audio.".into(),
        ));
    }
    // Publish rights follow the live roster: speakers publish; a host may always
    // publish (they grant themselves the floor implicitly).
    let roster = state.hub.stage_roster(room_id).unwrap_or_default();
    let can_publish = roster.speakers.contains(&user.id) || is_host(&state, &user, room_id).await;

    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let name = match user.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => user.username.clone(),
    };
    let token = livekit::mint_token(
        &cfg,
        livekit::Surface::Stage,
        room_id,
        &user.id,
        &name,
        can_publish,
        now,
    )
    .map_err(|e| AppError::Internal(format!("livekit token: {e}")))?;
    Ok(Json(StageTokenResponse {
        url: cfg.url,
        token,
        can_publish,
    }))
}

/// GET /room/{id}/huddle/token  (LC-596)
///
/// Mint a LiveKit token for the caller to join this room's huddle over the SFU.
///
/// Deliberately a separate route from the stage token rather than a mode flag on
/// it: the two surfaces have different gates (mesh membership vs the stage
/// roster), different publish rules (symmetric vs granted floor), and map to
/// different LiveKit rooms. Sharing one handler would mean branching on all
/// three inside it.
///
/// A huddle is symmetric, so every participant publishes - there is no listener
/// role to derive rights from, unlike the stage.
pub async fn get_huddle_token(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Json<StageTokenResponse>, AppError> {
    let Some(cfg) = LiveKitConfig::from_env() else {
        // Not an error condition: an operator with no LiveKit server is the
        // supported default, and the client falls back to the mesh on this.
        return Err(AppError::BadRequest(
            "Group call audio is not configured on this server.".into(),
        ));
    };
    let Some(room) = db::chat::get_room(&state.chat, room_id).await? else {
        return Err(AppError::NotFound);
    };
    // Huddles are the non-DM surface; a DM call is the 1:1 peer connection and
    // never routes through the SFU.
    if room.room_type == "dm" {
        return Err(AppError::BadRequest(
            "Direct-message calls do not use the SFU.".into(),
        ));
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    // The mesh roster is the gate, exactly as `require_participant` treats it
    // for transcription: being able to see the room is not being in the call.
    let participants = state.hub.voice_room_users(room_id);
    if !participants.iter().any(|u| u == &user.id) {
        return Err(AppError::BadRequest(
            "Join the huddle before connecting media.".into(),
        ));
    }
    // Below the threshold the mesh is still in charge, so refuse rather than
    // hand out a token that would silently split the call across two
    // transports - half the participants on the SFU, half on the mesh, hearing
    // nobody. The client asks again as the roster grows.
    if participants.len() < livekit::SFU_MIN_PARTICIPANTS {
        return Err(AppError::BadRequest(format!(
            "Huddle is below the SFU threshold ({} participants).",
            livekit::SFU_MIN_PARTICIPANTS
        )));
    }

    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let name = match user.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => user.username.clone(),
    };
    let token = livekit::mint_token(
        &cfg,
        livekit::Surface::Huddle,
        room_id,
        &user.id,
        &name,
        true,
        now,
    )
    .map_err(|e| AppError::Internal(format!("livekit token: {e}")))?;
    Ok(Json(StageTokenResponse {
        url: cfg.url,
        token,
        can_publish: true,
    }))
}
