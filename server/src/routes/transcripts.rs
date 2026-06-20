//! LC-393: call transcription endpoints. Phase 1 covered 1:1 DM calls; Phase 2
//! adds enclave voice channels (N participants).
//!
//! Calls are P2P (the server never sees audio), so each participant's browser
//! transcribes its OWN mic and POSTs final segments here. The server stores
//! them, broadcasts attributed live captions to the call's participants, and on
//! hangup finalizes the session + drops a linked "transcript saved" notice in
//! the room. The mutating endpoints (start/segment/end) are gated to active
//! participants: a DM member, or - for a voice channel - a user currently joined
//! to the channel (so a room member who never joined can neither start nor be
//! auto-captured). Live events are scoped to the actual participants.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::Form;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;
use crate::views::transcripts::{TranscriptLine, TranscriptPage};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// Display label for a user id, falling back to the username then the raw id.
async fn label_for(state: &AppState, user_id: &str) -> String {
    match db::auth::find_user_by_id(&state.auth, user_id).await {
        Ok(Some(u)) => u
            .display_name
            .filter(|d| !d.is_empty())
            .unwrap_or(u.username),
        _ => user_id.to_string(),
    }
}

/// Fetch the room and confirm it is a call-capable surface: a DM (1:1 calls,
/// LC-393 Phase 1) or a voice channel (`is_voice`, Phase 2). 404 anything else
/// so a non-call room is indistinguishable from "no such room".
async fn fetch_call_room(state: &AppState, room_id: i64) -> Result<Room, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type != "dm" && !room.is_voice {
        return Err(AppError::NotFound);
    }
    Ok(room)
}

/// Gate for mutating the live session (start / segment / end): the caller must
/// be an active PARTICIPANT, not merely able to see the room. A DM member is a
/// participant; for a voice channel the caller must currently be joined to the
/// channel (so a room member who never joined cannot start transcription and
/// silently auto-capture everyone who did).
async fn require_participant(state: &AppState, user: &User, room: &Room) -> Result<(), AppError> {
    let ok = if room.is_voice {
        state
            .hub
            .voice_room_users(room.id)
            .iter()
            .any(|u| u == &user.id)
    } else {
        db::chat::is_room_member(&state.chat, room.id, &user.id).await?
    };
    if ok {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Gate for reading a saved transcript: room access (a member can review it
/// after the fact, even once they've left a voice channel).
async fn require_access(state: &AppState, user: &User, room: &Room) -> Result<(), AppError> {
    let ok = if room.room_type == "dm" {
        db::chat::is_room_member(&state.chat, room.id, &user.id).await?
    } else {
        let is_admin = user.role == "admin";
        db::chat::is_room_accessible(&state.chat, room.id, &user.id, is_admin).await?
    };
    if ok {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// The set of users who should receive this call's live transcript events: the
/// voice channel's current participants for a voice room, else the DM's members.
/// Scoping voice broadcasts to actual participants is what keeps a room member
/// who never joined from being auto-captured.
async fn recipients(state: &AppState, room: &Room) -> Vec<String> {
    if room.is_voice {
        state.hub.voice_room_users(room.id)
    } else {
        db::chat::list_room_member_ids(&state.chat, room.id)
            .await
            .unwrap_or_default()
    }
}

/// Broadcast a per-recipient transcript event to each call participant (one
/// event per recipient, `to_user_id` set, the WS send task renders only for
/// that recipient - mirrors the call/voice signals).
async fn broadcast_to_members(
    state: &AppState,
    room: &Room,
    make_event: impl Fn(String) -> ChatEvent,
) {
    for m in recipients(state, room).await {
        let event = make_event(m.clone());
        state.hub.broadcast_to_user(&m, &event);
    }
}

/// POST /call/{room_id}/transcript/start
/// Open (or join) the call's transcription session and notify both members.
pub async fn start(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let room = fetch_call_room(&state, room_id).await?;
    require_participant(&state, &user, &room).await?;
    let session = db::transcripts::start_session(&state.chat, room_id, &user.id).await?;
    let by = label_for(&state, &user.id).await;
    let tid = session.id;
    broadcast_to_members(&state, &room, |to| ChatEvent::TranscriptStarted {
        room_id,
        to_user_id: to,
        transcript_id: tid,
        started_by_name: by.clone(),
    })
    .await;
    Ok(Json(json!({ "transcript_id": tid })))
}

#[derive(Deserialize)]
pub struct SegmentForm {
    pub text: String,
}

/// POST /call/transcript/{id}/segment
/// Append one finalized speech result and fan it out as a live caption.
pub async fn segment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
    Form(SegmentForm { text }): Form<SegmentForm>,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_participant(&state, &user, &room).await?;
    // A late segment after the session closed is benign - drop it silently so
    // the client never has to special-case the end race.
    let trimmed = text.trim();
    if session.status != "active" || trimmed.is_empty() {
        return Ok(Html(String::new()));
    }
    db::transcripts::append_segment(&state.chat, transcript_id, &user.id, trimmed).await?;
    let speaker = label_for(&state, &user.id).await;
    let body = trimmed.to_string();
    broadcast_to_members(&state, &room, |to| ChatEvent::TranscriptSegment {
        room_id: room.id,
        to_user_id: to,
        transcript_id,
        speaker_id: user.id.clone(),
        speaker_name: speaker.clone(),
        text: body.clone(),
    })
    .await;
    Ok(Html(String::new()))
}

/// POST /call/transcript/{id}/end
/// Close the session (idempotent), notify both members, and post the linked
/// "transcript saved" notice exactly once.
pub async fn end(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_participant(&state, &user, &room).await?;
    finalize(&state, &room, transcript_id).await;
    Ok(Html(String::new()))
}

/// Shared close path used by both the explicit /end and the WS-disconnect
/// backstop. `end_session` only transitions once, so the notice + end broadcast
/// fire exactly once even if both race.
async fn finalize(state: &AppState, room: &Room, transcript_id: i64) {
    let transitioned = db::transcripts::end_session(&state.chat, transcript_id)
        .await
        .unwrap_or(false);
    broadcast_to_members(state, room, |to| ChatEvent::TranscriptEnded {
        room_id: room.id,
        to_user_id: to,
        transcript_id,
    })
    .await;
    if transitioned {
        post_saved_message(state, room, transcript_id).await;
    }
}

/// LC-393 Phase 2 backstop: when a voice channel empties, finalize any session
/// still open for it (save + post the notice) - the equivalent of the per-user
/// disconnect backstop for the shared-channel case.
pub async fn finalize_open_for_room(state: &AppState, room_id: i64) {
    let Ok(Some(session)) = db::transcripts::open_session_for_room(&state.chat, room_id).await
    else {
        return;
    };
    if let Ok(room) = fetch_call_room(state, room_id).await {
        finalize(state, &room, session.id).await;
    }
}

/// LC-393 backstop: a hard WS drop never sends /end, so close any session the
/// dropped user started and finalize it (mirrors LC-186's remote-control
/// backstop). Called from the WS disconnect cleanup.
pub async fn finalize_open_for_user(state: &AppState, user_id: &str) {
    let closed = db::transcripts::end_open_sessions_started_by(&state.chat, user_id)
        .await
        .unwrap_or_default();
    for tid in closed {
        if let Ok(Some(session)) = db::transcripts::get(&state.chat, tid).await {
            if let Ok(Some(room)) = db::chat::get_room(&state.chat, session.room_id).await {
                // end_session already transitioned (it's in `closed`), so just
                // broadcast end + post the notice.
                broadcast_to_members(state, &room, |to| ChatEvent::TranscriptEnded {
                    room_id: room.id,
                    to_user_id: to,
                    transcript_id: tid,
                })
                .await;
                post_saved_message(state, &room, tid).await;
            }
        }
    }
}

/// Insert + broadcast the "transcript saved" system message linking to the
/// saved transcript (mirrors `post_call_started_message`).
async fn post_saved_message(state: &AppState, room: &Room, transcript_id: i64) {
    // Author the system row as whoever started the session (a real user id, so
    // avatar/name resolution behaves), defaulting to the room if unknown.
    let author_id = match db::transcripts::get(&state.chat, transcript_id).await {
        Ok(Some(t)) => t.started_by,
        _ => return,
    };
    let body = format!("[\u{1F4DD} Call transcript saved](/transcripts/{transcript_id})");
    let new_id =
        match db::chat::insert_system_message(&state.chat, room.id, &author_id, &body).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "failed to insert transcript-saved message");
                return;
            }
        };
    // Author label is cosmetic for a system row; use the saved id's room. Build
    // the broadcast message the same way the call-started notice does.
    let raw = match db::chat::get_message(&state.chat, new_id).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let author_name = label_for(state, &raw.user_id).await;
    let message = crate::models::Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name,
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
        email_inbox_id: raw.email_inbox_id,
        bridge_id: raw.bridge_id,
        bridge_foreign_name: raw.bridge_foreign_name,
        bridge_kind: raw.bridge_kind,
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };
    let event = ChatEvent::NewMessage {
        message,
        is_dm: true,
        client_id: None,
    };
    if let Err(e) = super::broadcast_room_message(state, room, &event).await {
        tracing::warn!(error = %e, "failed to broadcast transcript-saved message");
    }
}

/// GET /transcripts/{id}
/// The saved transcript page, gated to DM members.
pub async fn show(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_access(&state, &user, &room).await?;
    let segments = db::transcripts::list_segments(&state.chat, transcript_id).await?;

    let mut names: HashMap<String, String> = HashMap::new();
    let mut lines = Vec::with_capacity(segments.len());
    for s in segments {
        let speaker_name = match names.get(&s.user_id) {
            Some(n) => n.clone(),
            None => {
                let n = label_for(&state, &s.user_id).await;
                names.insert(s.user_id.clone(), n.clone());
                n
            }
        };
        lines.push(TranscriptLine {
            speaker_name,
            text: s.text,
            spoken_at: s.spoken_at,
        });
    }

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;

    html(&TranscriptPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        transcript_id,
        room_name: room.name,
        started_at: session.started_at,
        ended: session.status != "active",
        lines,
    })
}
