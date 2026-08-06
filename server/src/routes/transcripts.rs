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

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Form;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;
use crate::views::transcripts::{
    ToastOob, TranscriptIndexPage, TranscriptIndexRow, TranscriptLine, TranscriptListBody,
    TranscriptPage,
};
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
/// LC-393 Phase 1), a voice channel (`is_voice`, Phase 2), or a group room
/// hosting an ad-hoc huddle (LC-553).
///
/// A huddle is the SAME WebRTC mesh as a voice channel (voice.js, keyed to the
/// text room's id) with the same per-participant mic capture transcription
/// relies on - it was excluded only because its room is not flagged `is_voice`,
/// so captions worked in DMs and voice channels but silently 404'd in huddles.
/// Every group room is huddle-capable (`huddle_enabled` is just
/// `room_type != "dm"`), so the surface check is now just "a real room"; actual
/// authority to open a session is enforced by `require_participant` below, which
/// demands active mesh membership rather than mere room access.
async fn fetch_call_room(state: &AppState, room_id: i64) -> Result<Room, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(room)
}

/// Gate for mutating the live session (start / segment / end): the caller must
/// be an active PARTICIPANT, not merely able to see the room. A DM member is a
/// participant; for a voice channel OR a huddle the caller must currently be
/// joined to the mesh (so a room member who never joined cannot start
/// transcription and silently auto-capture everyone who did).
///
/// LC-553: this keyed on `is_voice`, which sent huddles (a non-`is_voice` group
/// room) down the DM branch, where plain room membership would have been enough.
/// Now anything that is not a DM is gated on live mesh membership - the hub keys
/// huddles by the text room's id, so `voice_room_users` covers both.
///
/// LC-597: a Stage carries its audio over the LiveKit SFU, not the mesh, so a
/// Stage speaker is absent from `voice_room_users` and was refused here. The
/// mesh check runs first because it is in-memory and covers every other
/// surface; only a caller it rejects pays for the stage lookup.
async fn require_participant(state: &AppState, user: &User, room: &Room) -> Result<(), AppError> {
    let ok = if room.room_type == "dm" {
        db::chat::is_room_member(&state.chat, room.id, &user.id).await?
    } else {
        state
            .hub
            .voice_room_users(room.id)
            .iter()
            .any(|u| u == &user.id)
            || is_stage_speaker(state, room, user).await?
    };
    if ok {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// LC-597: does `user` currently hold the floor on `room`'s stage?
///
/// Listeners are deliberately excluded: a Stage is a broadcast format, so the
/// audience is large and only the people actually speaking are captured. That
/// keeps the capture set equal to the set of published tracks.
///
/// The `stage_enabled` re-check is not redundant with the join path. The roster
/// lives in the hub (`Hub::stages`) and is only mutated by the WS stage frames;
/// turning the stage off in room settings clears neither. Without this a host
/// who disabled the stage would leave every user who held the floor at that
/// moment able to keep opening sessions and posting segments.
async fn is_stage_speaker(state: &AppState, room: &Room, user: &User) -> Result<bool, AppError> {
    let holds_floor = state
        .hub
        .stage_roster(room.id)
        .is_some_and(|r| r.speakers.contains(&user.id));
    if !holds_floor {
        return Ok(false);
    }
    Ok(db::chat::get_room_stage_enabled(&state.chat, room.id).await?)
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

/// LC-629: how many recent (already-corrected) lines feed the correction pass
/// as context. Bounds token cost per the ticket's rolling-window requirement -
/// correction sees the last few lines, never the whole session.
const CORRECTION_CONTEXT_LINES: i64 = 6;

/// LC-629: best-effort AI correction of one freshly recognized line against the
/// recent conversation. Returns the corrected DISPLAY text, or the raw text
/// unchanged when no LLM is configured or the call fails - a live caption must
/// never be dropped or blocked because correction was unavailable, and the raw
/// recognition is preserved by the caller regardless.
async fn correct_line(state: &AppState, transcript_id: i64, raw: &str) -> String {
    let Some(llm) = state.llm_client.clone() else {
        return raw.to_string();
    };
    // LC-679: kill switch silences live transcript correction too (flag-only;
    // per-segment system path with no user). Off -> raw recognition passes through.
    if !super::ai_gate::flag_on(state).await {
        return raw.to_string();
    }
    let context =
        db::transcripts::recent_segment_texts(&state.chat, transcript_id, CORRECTION_CONTEXT_LINES)
            .await
            .unwrap_or_default();
    match llm.correct(&context, raw).await {
        Ok(corrected) => {
            let corrected = corrected.trim();
            if corrected.is_empty() {
                raw.to_string()
            } else {
                corrected.to_string()
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "transcript correction failed; using raw recognition");
            raw.to_string()
        }
    }
}

/// Store one final speech result (trimmed, length-capped in the db layer) and
/// fan it out as an attributed live caption. Shared by the browser-text path
/// (`segment`) and the server-STT path (`audio`). A blank result is a no-op.
///
/// LC-629: the recognized text is first passed through a best-effort correction
/// pass against a rolling window of recent lines; the CORRECTED text is what is
/// displayed and broadcast, while the original recognition is preserved as the
/// segment's `raw_text` (NULL when correction was off, failed, or was a no-op).
async fn record_and_broadcast(
    state: &AppState,
    room: &Room,
    transcript_id: i64,
    user: &User,
    text: &str,
    duration_ms: i64,
) -> Result<(), AppError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let corrected = correct_line(state, transcript_id, trimmed).await;
    // Keep the raw recognition only when correction actually changed it.
    let raw_text = (corrected != trimmed).then_some(trimmed);
    db::transcripts::append_segment(
        &state.chat,
        transcript_id,
        &user.id,
        &corrected,
        raw_text,
        duration_ms,
    )
    .await?;
    let speaker = label_for(state, &user.id).await;
    broadcast_to_members(state, room, |to| ChatEvent::TranscriptSegment {
        room_id: room.id,
        to_user_id: to,
        transcript_id,
        speaker_id: user.id.clone(),
        speaker_name: speaker.clone(),
        text: corrected.clone(),
    })
    .await;
    Ok(())
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
    if session.status != "active" {
        return Ok(Html(String::new()));
    }
    // Browser Web Speech path: text only, no engine timings (duration 0 -> the
    // export uses its synthetic cue length for these).
    record_and_broadcast(&state, &room, transcript_id, &user, &text, 0).await?;
    Ok(Html(String::new()))
}

/// POST /call/transcript/{id}/audio
/// LC-393 Phase 3: server-side STT. Accepts one short audio clip (raw body, the
/// browser's MediaRecorder webm/opus), forwards it to the operator's configured
/// STT endpoint, and records the returned text as a segment + live caption -
/// same persistence/broadcast path as `segment`, just a different producer.
/// Gated to participants; 400 when server STT is not configured.
pub async fn audio(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_participant(&state, &user, &room).await?;
    let Some(stt) = state.stt_client.clone() else {
        return Err(AppError::BadRequest(
            "server-side transcription is not configured".into(),
        ));
    };
    // Late clip after the session closed, or an empty body: drop silently.
    if session.status != "active" || body.is_empty() {
        return Ok(Html(String::new()));
    }
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/webm")
        .to_string();
    // LC-591: hint the engine with the speaker's preferred locale (the clip is
    // this user's own mic). Rooms carry no locale, so the user's is the signal;
    // None lets the engine autodetect.
    let language = user.locale.as_deref();
    // LC-590: retry transient failures, and on exhaustion answer non-2xx so the
    // client can show a caption-failed indicator. Pre-LC-590 this returned 200
    // with an empty body, which is indistinguishable from "you were silent" -
    // the operator saw a log line and the user saw nothing at all.
    //
    // The clip is still dropped: a call caption is live text, so there is
    // nothing useful to replay by the time three attempts have failed. The
    // capture loop keeps running, so recovery is automatic on the next clip.
    // LC-592: live captions are the heaviest producer (one clip per 5 seconds
    // per speaker), so they are rate-limited alongside stored attachments.
    if !crate::stt_load::try_admit(&state.rate_limits, room.id) {
        tracing::info!(room_id = room.id, "stt rate limited; dropping clip");
        return Err(AppError::TooManyRequests(
            "transcription rate limit".into(),
            60,
        ));
    }
    // LC-592: concurrency. Unlike the voice-message path this does NOT wait for
    // a permit - it SHEDS. A caption is latency-bound and worthless once it
    // arrives late, and the next clip is already 5 seconds away, so queueing
    // behind a batch of long voice notes would deliver stale captions AND hold
    // the engine longer. Dropping now surfaces as LC-590's caption warning and
    // recovers on the next clip.
    let Ok(_permit) = crate::stt_load::permits().try_acquire() else {
        tracing::info!(room_id = room.id, "stt at capacity; dropping clip");
        return Err(AppError::TooManyRequests("transcription is busy".into(), 5));
    };
    let req = crate::stt::SttRequest::new(body.to_vec(), content_type)
        .with_language(language)
        .with_timeout_secs(crate::stt::LIVE_CLIP_TIMEOUT_SECS);
    let result = match crate::stt::transcribe_with_retry(stt.as_ref(), req).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "stt transcription failed after retries; dropping clip");
            return Err(AppError::Internal("transcription failed".into()));
        }
    };
    // LC-591: store the real spoken length from the engine's segment timings so
    // the WebVTT export can use it instead of a synthetic cue duration.
    record_and_broadcast(
        &state,
        &room,
        transcript_id,
        &user,
        &result.text,
        result.duration_ms(),
    )
    .await?;
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
        // LC-662: kick off the async AI recap (opt-in, best-effort).
        maybe_post_call_recap(state, room, transcript_id);
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
                // LC-662: kick off the async AI recap (opt-in, best-effort).
                maybe_post_call_recap(state, &room, tid);
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

/// GET /transcripts
/// LC-394: the archive of every transcript the viewer can access (DM calls +
/// voice channels they belong to), newest first. Each candidate is gated with
/// the same `is_room_accessible` check the single-transcript page uses, so no
/// transcript from a room the viewer cannot see is ever listed.
/// LC-440: build the visible archive rows for `user`, honoring the search query
/// (matches the resolved title / date / transcript content) and the hide-empty
/// filter. Each candidate is gated with the same `is_room_accessible` check the
/// single-transcript page uses, so no transcript from a room the viewer cannot
/// see is ever listed. `can_delete` follows the LC-441 rule: the starter or a
/// site admin.
async fn build_index_rows(
    state: &AppState,
    user: &User,
    query: &str,
    hide_empty: bool,
) -> Result<Vec<TranscriptIndexRow>, AppError> {
    const SCAN: i64 = 300;
    const SHOW: usize = 100;
    let is_admin = user.role == "admin";
    let q_lower = query.to_lowercase();
    // Content-match ids (LC-395 SQL search), so a query also matches transcript
    // body text - not just the title/date we resolve below.
    let content_ids: std::collections::HashSet<i64> = if query.is_empty() {
        std::collections::HashSet::new()
    } else {
        db::transcripts::list_recent(&state.chat, Some(query), SCAN)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect()
    };
    let candidates = db::transcripts::list_recent(&state.chat, None, SCAN).await?;

    let mut rows: Vec<TranscriptIndexRow> = Vec::new();
    for c in candidates {
        if rows.len() >= SHOW {
            break;
        }
        if hide_empty && c.segment_count == 0 {
            continue;
        }
        if !db::chat::is_room_accessible(&state.chat, c.room_id, &user.id, is_admin).await? {
            continue;
        }
        // DM: headline the peer (the other member); voice: the channel name.
        let is_dm = c.room_type == "dm";
        let (title, kind_label) = if is_dm {
            let members = db::chat::list_room_member_ids(&state.chat, c.room_id)
                .await
                .unwrap_or_default();
            let peer = members.into_iter().find(|m| m != &user.id);
            let title = match peer {
                Some(pid) => label_for(state, &pid).await,
                None => crate::i18n::translate_current("transcript-kind-dm"),
            };
            (title, crate::i18n::translate_current("transcript-kind-dm"))
        } else {
            let name = if c.room_name.trim().is_empty() {
                crate::i18n::translate_current("transcript-kind-voice")
            } else {
                c.room_name.clone()
            };
            (
                name,
                crate::i18n::translate_current("transcript-kind-voice"),
            )
        };
        // LC-445: a query matches the resolved title, the date, or the content.
        if !query.is_empty()
            && !title.to_lowercase().contains(&q_lower)
            && !c.started_at.to_lowercase().contains(&q_lower)
            && !content_ids.contains(&c.id)
        {
            continue;
        }
        // LC-441: only the starter or a site admin may delete a shared record.
        let can_delete = is_admin || can_delete_transcript(&state.chat, c.id, &user.id).await;
        rows.push(TranscriptIndexRow {
            id: c.id,
            title,
            kind_label,
            is_dm,
            started_at: c.started_at,
            ended: c.status != "active",
            segment_count: c.segment_count,
            can_delete,
        });
    }
    Ok(rows)
}

/// LC-441: the destructive-delete predicate beyond view access - the user who
/// started the transcript (or, at the call site, a site admin) may delete it.
async fn can_delete_transcript(chat: &sqlx::SqlitePool, id: i64, user_id: &str) -> bool {
    matches!(db::transcripts::get(chat, id).await, Ok(Some(t)) if t.started_by == user_id)
}

pub async fn index(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(IndexQuery { q, hide_empty }): Query<IndexQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default().trim().to_string();
    let hide_empty = hide_empty.is_some();
    let rows = build_index_rows(&state, &user, &query, hide_empty).await?;

    // LC-445: htmx search / filter swaps just the list body.
    if headers.contains_key("hx-request") {
        return html(&TranscriptListBody {
            rows,
            query,
            hide_empty,
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

    html(&TranscriptIndexPage {
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
        rows,
        query,
        hide_empty,
    })
}

/// LC-441: DELETE /transcripts/{id}. Removes a saved transcript (+ its segments).
/// Authz: the same read-access gate (`require_access`) AND the destructive rule -
/// only the user who started it, or a site admin, may delete a shared record.
/// Returns an empty body (htmx swaps the row out) plus an out-of-band toast.
pub async fn delete_transcript(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
) -> Result<Response, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_access(&state, &user, &room).await?;
    if user.role != "admin" && session.started_by != user.id {
        return Err(AppError::Forbidden);
    }
    db::transcripts::delete(&state.chat, transcript_id).await?;
    let toast = html(&ToastOob {
        ok: true,
        message: crate::i18n::translate_current("transcript-deleted"),
    })?;
    Ok(toast.into_response())
}

/// LC-442: DELETE /transcripts - bulk-delete the viewer's empty (0-line)
/// transcripts. Only rows the viewer may delete (started, or admin) and can
/// access are removed. Returns the re-rendered list body + a toast with the count.
pub async fn delete_empty(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(IndexQuery { q, hide_empty }): Query<IndexQuery>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    let mut deleted = 0i64;
    for c in db::transcripts::list_recent(&state.chat, None, 300).await? {
        if c.segment_count != 0 || c.status == "active" {
            continue;
        }
        if !db::chat::is_room_accessible(&state.chat, c.room_id, &user.id, is_admin).await? {
            continue;
        }
        let owner = matches!(db::transcripts::get(&state.chat, c.id).await, Ok(Some(t)) if t.started_by == user.id);
        if !is_admin && !owner {
            continue;
        }
        if db::transcripts::delete(&state.chat, c.id).await? {
            deleted += 1;
        }
    }
    // Re-render the (now-trimmed) list under the current search/filter, plus a
    // toast reporting how many were removed.
    let query = q.unwrap_or_default().trim().to_string();
    let hide_empty = hide_empty.is_some();
    let rows = build_index_rows(&state, &user, &query, hide_empty).await?;
    let body = html(&TranscriptListBody {
        rows,
        query,
        hide_empty,
    })?;
    let toast = html(&ToastOob {
        ok: true,
        message: crate::i18n::translate_current("transcript-deleted-n")
            .replace("%count%", &deleted.to_string()),
    })?;
    Ok(Html(format!("{}{}", body.0, toast.0)))
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

    // LC-396: render the cached summary markdown (if any) to HTML.
    let summary_html = session
        .summary
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|md| crate::views::markdown::render(md, &[], &[]));
    // LC-527: offer the "Create follow-up tasks" button only when the summary
    // has parseable action items.
    let has_action_items = session
        .summary
        .as_deref()
        .map(|md| !super::followups::parse_action_items(md).is_empty())
        .unwrap_or(false);

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

    // LC-679: role-scoped AI gate for the transcript "Summarize" action.
    let ai_flag_on = super::ai_gate::flag_on(&state).await;
    let ai_llm = ai_flag_on
        && state.llm_available()
        && super::ai_gate::privileged_in_room(&state, room.id, &user).await?;
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
        llm_available: ai_llm,
        llm_teaser: !state.llm_available() && user.role == "admin",
        summary_html,
        has_action_items,
    })
}

/// LC-396/LC-662: assemble the `Speaker: text` transcript prompt for the model,
/// bounded so the prompt stays a reasonable size. Shared by the on-demand summary
/// handler and the automatic post-call recap.
async fn build_transcript_prompt_text(
    state: &AppState,
    transcript_id: i64,
) -> Result<String, AppError> {
    const MAX_PROMPT_CHARS: usize = 48_000;
    let segments = db::transcripts::list_segments(&state.chat, transcript_id).await?;
    // Resolve a display name per distinct speaker, preserving first-appearance
    // order. LC-663: a leading `Participants:` roster gives the model the exact
    // name set to attribute decisions + action items against.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for s in &segments {
        if !names.contains_key(&s.user_id) {
            names.insert(s.user_id.clone(), label_for(state, &s.user_id).await);
            order.push(s.user_id.clone());
        }
    }
    let mut text = String::new();
    if !order.is_empty() {
        let roster: Vec<&str> = order.iter().map(|id| names[id].as_str()).collect();
        text.push_str("Participants: ");
        text.push_str(&roster.join(", "));
        text.push_str("\n\n");
    }
    for s in &segments {
        text.push_str(&names[&s.user_id]);
        text.push_str(": ");
        text.push_str(&s.text);
        text.push('\n');
        if text.len() >= MAX_PROMPT_CHARS {
            break;
        }
    }
    Ok(text)
}

/// LC-662: after a call transcript finalizes, post an AI recap (summary + action
/// items) into the room as the `assistant` bot, so nobody has to open the
/// transcript page and click Summarize. Gated on `rooms.assistant_enabled` - the
/// same manager opt-in that governs the /ask assistant, since this is the same
/// bot posting in the same room. Fire-and-forget: local summarization can take
/// many seconds and nobody is waiting on it, so a failure only logs.
fn maybe_post_call_recap(state: &AppState, room: &Room, transcript_id: i64) {
    let state = state.clone();
    let room = room.clone();
    tokio::spawn(async move {
        if let Err(e) = post_call_recap(&state, &room, transcript_id).await {
            tracing::warn!(error = %e, transcript_id, "auto call recap failed");
        }
    });
}

/// The recap body of [`maybe_post_call_recap`]. Idempotent: the transcript's
/// stored `summary` doubles as a "recap already produced" marker, so a
/// re-finalize race (explicit /end plus the WS-disconnect backstop) never
/// double-posts. Silent no-op when no LLM is configured, the room has not opted
/// in, or the call produced no speech.
async fn post_call_recap(
    state: &AppState,
    room: &Room,
    transcript_id: i64,
) -> Result<(), AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Ok(());
    };
    // LC-679: the runtime kill switch also silences this auto-recap (no user
    // context here, so it is flag-gated only, like the other background jobs).
    if !super::ai_gate::flag_on(state).await {
        return Ok(());
    }
    if !db::chat::get_room_assistant_enabled(&state.chat, room.id).await? {
        return Ok(());
    }
    // Dedupe: a stored summary means the recap already ran for this transcript.
    let Some(session) = db::transcripts::get(&state.chat, transcript_id).await? else {
        return Ok(());
    };
    if session
        .summary
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty())
    {
        return Ok(());
    }

    let text = build_transcript_prompt_text(state, transcript_id).await?;
    if text.trim().is_empty() {
        return Ok(());
    }
    let md = match llm.summarize(&text).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, transcript_id, "auto call recap summarization failed");
            return Ok(());
        }
    };
    // Store first so it doubles as the dedupe marker even if the post fails.
    db::transcripts::set_summary(&state.chat, transcript_id, &md).await?;

    let bot = super::assistant::assistant_bot(state).await?;
    let body = format!(
        "**\u{1F4DD} Call recap** \u{00B7} [full transcript](/transcripts/{transcript_id})\n\n{md}"
    );
    let new_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &body).await?;
    super::room::finalize_message_send(state, room, &bot, new_id, &body, None).await?;
    Ok(())
}

/// POST /transcripts/{id}/summary
/// LC-396: generate (or regenerate) the transcript's LLM summary, store it, and
/// return the rendered summary fragment. Gated like the transcript page; 400
/// when no LLM endpoint is configured.
pub async fn summary(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_access(&state, &user, &room).await?;
    // LC-679: runtime flag + role gate (403 for unprivileged/flag-off).
    super::ai_gate::require_llm_in_room(&state, room.id, &user).await?;
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "transcript summarization is not configured".into(),
        ));
    };

    let text = build_transcript_prompt_text(&state, transcript_id).await?;
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("transcript is empty".into()));
    }

    let md = match llm.summarize(&text).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "transcript summarization failed");
            return Err(AppError::BadRequest("summarization failed".into()));
        }
    };
    db::transcripts::set_summary(&state.chat, transcript_id, &md).await?;
    let summary_html = crate::views::markdown::render(&md, &[], &[]);
    let has_action_items = !super::followups::parse_action_items(&md).is_empty();
    html(&crate::views::transcripts::TranscriptSummaryFragment {
        transcript_id,
        summary_html,
        has_action_items,
    })
}

/// LC-664: the personal-brief system prompt. Frames the transcript as data and
/// asks for a short, second-person brief of only what concerns the named viewer.
const BRIEF_SYSTEM: &str = "You brief one participant on a call they may have missed or joined \
late. You are given the call transcript (each line is 'Name: what they said', led by a \
'Participants:' line) and, above it, the NAME of the person to brief. Address that person directly \
as 'you' and tell them only what matters to them: decisions that were made, any action items, \
requests, or questions directed at them, and anything that mentioned or concerns them. Be concise - \
a few short bullets. If nothing in the call specifically concerns them, say so plainly in one line \
and give a one-sentence gist of the call. Use only what is in the transcript; never invent people, \
names, or details.";

/// POST /transcripts/{id}/brief
/// LC-664: a per-viewer "what did I miss" brief - decisions, action items aimed
/// at the viewer, and anything that mentioned or concerns them. Gated like the
/// summary page (any member can read it). Personalized to the viewer, so unlike
/// the shared summary it is NOT cached: it is per-viewer and cheap to regenerate,
/// the same posture as the chat catch-me-up (LC-484).
pub async fn brief(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_access(&state, &user, &room).await?;
    // LC-679: runtime flag + role gate (403 for unprivileged/flag-off).
    super::ai_gate::require_llm_in_room(&state, room.id, &user).await?;
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "transcript summarization is not configured".into(),
        ));
    };

    let text = build_transcript_prompt_text(&state, transcript_id).await?;
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("transcript is empty".into()));
    }
    // The viewer's name is DATA (the guardrail keeps it from being treated as an
    // instruction), so the model knows who "you" refers to.
    let name = label_for(&state, &user.id).await;
    let user_content = format!("Person to brief: {name}\n\n{text}");
    let md = match llm.complete_guarded(BRIEF_SYSTEM, &user_content).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "transcript brief failed");
            return Err(AppError::BadRequest("brief failed".into()));
        }
    };
    let brief_html = crate::views::markdown::render(&md, &[], &[]);
    html(&crate::views::transcripts::TranscriptBriefFragment {
        transcript_id,
        brief_html,
    })
}

/// POST /transcripts/{id}/follow-ups
/// LC-527: turn the transcript summary's "## Action items" into a trackable
/// checklist posted to the call's room. Gated like the summary; 400 when the
/// summary has no action items yet. Returns a small confirmation fragment.
pub async fn create_followups(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
) -> Result<Html, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_access(&state, &user, &room).await?;

    let items = super::followups::parse_action_items(session.summary.as_deref().unwrap_or(""));
    if items.is_empty() {
        return Err(AppError::BadRequest(
            "this transcript has no action items to turn into tasks".into(),
        ));
    }
    let title = crate::i18n::translate_current("followup-card-title");
    let message_id = db::followups::create(
        &state.chat,
        room.id,
        &user.id,
        &title,
        Some(transcript_id),
        &items,
    )
    .await?;
    super::room::finalize_message_send(&state, &room, &user, message_id, &title, None).await?;
    html(&crate::views::transcripts::FollowUpsCreatedFragment {})
}

#[derive(Deserialize)]
pub struct IndexQuery {
    #[serde(default)]
    pub q: Option<String>,
    /// LC-442: present ("1") to hide 0-line transcripts.
    #[serde(default)]
    pub hide_empty: Option<String>,
}

#[derive(Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: Option<String>,
    /// LC-629: present ("1") to export the RAW, uncorrected recognition instead
    /// of the AI-corrected display text - the preserved record is retrievable.
    #[serde(default)]
    pub raw: Option<String>,
}

/// Parse a `datetime('now')` string ("%Y-%m-%d %H:%M:%S").
fn parse_dt(s: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
}

/// Seconds -> "HH:MM:SS".
fn fmt_clock(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// Seconds -> WebVTT "HH:MM:SS.mmm".
fn fmt_vtt(secs: f64) -> String {
    let s = secs.max(0.0);
    let total = s as u64;
    let ms = ((s - total as f64) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        total / 3600,
        (total % 3600) / 60,
        total % 60,
        ms
    )
}

/// GET /transcripts/{id}/export?format=txt|vtt
/// LC-395: download a saved transcript as plain text or WebVTT. Gated like the
/// transcript page; served as an attachment.
pub async fn export(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(transcript_id): Path<i64>,
    Query(ExportQuery { format, raw }): Query<ExportQuery>,
) -> Result<Response, AppError> {
    let session = db::transcripts::get(&state.chat, transcript_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room = fetch_call_room(&state, session.room_id).await?;
    require_access(&state, &user, &room).await?;
    let want_raw = raw.is_some();
    let segments = db::transcripts::list_segments(&state.chat, transcript_id).await?;

    // Resolve speaker labels (cache) and each segment's offset from the call
    // start, in seconds.
    let start = parse_dt(&session.started_at);
    let mut names: HashMap<String, String> = HashMap::new();
    // (offset_secs, speaker, text, duration_ms) - LC-591 threads the real
    // per-segment duration through for the VTT cue length.
    let mut lines: Vec<(f64, String, String, i64)> = Vec::with_capacity(segments.len());
    for s in &segments {
        let name = match names.get(&s.user_id) {
            Some(n) => n.clone(),
            None => {
                let n = label_for(&state, &s.user_id).await;
                names.insert(s.user_id.clone(), n.clone());
                n
            }
        };
        let off = match (start, parse_dt(&s.spoken_at)) {
            (Some(a), Some(b)) => (b - a).num_milliseconds().max(0) as f64 / 1000.0,
            _ => 0.0,
        };
        // LC-629: raw export emits the uncorrected recognition; the default
        // emits the AI-corrected display text.
        let text = if want_raw {
            s.raw().to_string()
        } else {
            s.text.clone()
        };
        lines.push((off, name, text, s.duration_ms));
    }

    let vtt = format.as_deref() == Some("vtt");
    let (body, content_type, ext) = if vtt {
        (build_vtt(&lines), "text/vtt; charset=utf-8", "vtt")
    } else {
        (
            build_txt(&room.name, &session.started_at, &lines),
            "text/plain; charset=utf-8",
            "txt",
        )
    };
    let headers = [
        (axum::http::header::CONTENT_TYPE, content_type.to_string()),
        (
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"transcript-{transcript_id}.{ext}\""),
        ),
    ];
    Ok((headers, body).into_response())
}

/// Plain-text transcript: a header then `HH:MM:SS  Speaker: text` per line.
fn build_txt(room_name: &str, started_at: &str, lines: &[(f64, String, String, i64)]) -> String {
    let mut out = format!("Call transcript - {room_name} - {started_at}\n\n");
    for (off, speaker, text, _dur) in lines {
        out.push_str(&format!("{}  {}: {}\n", fmt_clock(*off), speaker, text));
    }
    out
}

/// WebVTT transcript. Cue times are forced monotonic (the stored timestamps are
/// only second-resolution, so several segments can share an offset): each cue
/// starts at `max(real_offset, prev_end)`. Its END is the real spoken duration
/// (LC-591, from the engine's segment timings) when known, else the pre-LC-591
/// synthetic length: the next cue's start, floored to keep the cue non-zero.
/// Either way cues stay valid, ordered, and non-overlapping.
fn build_vtt(lines: &[(f64, String, String, i64)]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    let mut cursor = 0.0_f64;
    for (i, (off, speaker, text, dur_ms)) in lines.iter().enumerate() {
        let start = off.max(cursor);
        // Real duration when the engine gave us one; otherwise fall back to the
        // synthetic "until the next cue" length. Never let the next cue start
        // before this one really ends, so a real duration still can't overlap.
        let end = if *dur_ms > 0 {
            start + (*dur_ms as f64) / 1000.0
        } else {
            let next = lines
                .get(i + 1)
                .map(|(o, _, _, _)| *o)
                .unwrap_or(start + 3.0);
            next.max(start + 2.0)
        };
        cursor = end;
        // Escape the VTT-significant characters in the cue payload.
        let safe = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let safe_speaker = speaker
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        out.push_str(&format!(
            "{} --> {}\n<v {}>{}\n\n",
            fmt_vtt(start),
            fmt_vtt(end),
            safe_speaker,
            safe
        ));
    }
    out
}
