//! Per-room message drafts: server-persisted in-progress textarea
//! contents.
//!
//! Single endpoint: `PUT /room/{room_id}/draft` accepts a form-encoded
//! `body` field. Four handler paths, each with a distinct status:
//!
//! - Room not visible to the caller -> 403. Drafts piggyback on the
//!   existing `is_room_accessible` predicate; a user cannot create a
//!   draft in a room they could not also post in.
//! - Body trims to empty -> DELETE the row, 204.
//! - Body matches a recent `messages` row from this user in this room
//!   (race guard against send-then-resurrect) -> no-op, 204.
//! - Otherwise -> UPSERT the row, 204.
//!
//! The body is parsed via `axum::Form<DraftForm>` (consistent with every
//! other text-content handler in this codebase). The textarea in
//! composer.html carries `name="body"`, so `hx-include="this"` on the
//! HTMX PUT form-encodes correctly.
//!
//! The handler always returns 204 (no body, no fragment). The textarea
//! on the client already has the latest text; nothing to render back.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Form,
};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::ws::events::ChatEvent;

/// LC-239: tell every one of `user_id`'s tabs to swap the sidebar draft
/// indicator for `room_id`. Per-user fan-out; tabs whose current page lacks
/// that sidebar row drop the OOB swap silently.
fn broadcast_draft_changed(state: &AppState, user_id: &str, room_id: i64, has_draft: bool) {
    state.hub.broadcast_to_user(
        user_id,
        &ChatEvent::DraftChanged {
            user_id: user_id.to_string(),
            room_id,
            has_draft,
        },
    );
}

#[derive(Deserialize)]
pub struct DraftForm {
    /// Raw textarea contents as submitted. The handler trims before
    /// any work (race guard SELECT, delete-on-empty, upsert) so the
    /// stored body matches the send path's already-trimmed shape.
    pub body: String,
}

/// `PUT /room/{room_id}/draft`
pub async fn put_draft(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<DraftForm>,
) -> Result<impl IntoResponse, AppError> {
    // Visibility predicate is the same one the send path uses
    // (routes/room.rs::post_message at line 437). A user who cannot
    // post in this room cannot draft in it either; otherwise drafts
    // become a probe for room existence / membership across the
    // accessibility boundary.
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // Trim the draft body to match what the send path (routes/room.rs:
    // post_message at line 397) does to the message body before it
    // hits `messages`. This is one-sided alignment: the draft side
    // trims here, and the race-guard SELECT below compares the
    // trimmed draft against `messages.body` (which is already stored
    // trimmed). Do NOT trim inside the SQL; messages.body is the
    // already-trimmed canonical form, not a column that needs further
    // normalization at compare time.
    let body = form.body.trim();

    if body.is_empty() {
        // Only signal the sidebar when a row actually went away; an empty
        // PUT for a room that never had a draft is a no-op the badge already
        // reflects.
        if db::drafts::delete(&state.chat, &user.id, room_id)
            .await
            .unwrap_or(0)
            > 0
        {
            broadcast_draft_changed(&state, &user.id, room_id, false);
        }
        return Ok(StatusCode::NO_CONTENT);
    }

    // Race guard: send clears the draft in finalize_message_send, but
    // a debounced keyup that fired just before Enter (or whose
    // trailing fire lands after the send commit) would otherwise
    // resurrect the row with the just-sent body. Reject the upsert
    // when the trimmed body exactly matches a `messages` row from
    // this user in this room created within the last 5 seconds.
    //
    // Acceptable false-positive: re-typing the same short message
    // within 5 seconds of sending it drops the new draft. Rare;
    // recoverable (re-type after the window). Cheaper than a perfect
    // client-side cancellation of any in-flight PUT at send time.
    let recent_match: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM messages \
         WHERE user_id = ? \
           AND room_id = ? \
           AND body = ? \
           AND created_at >= datetime('now', '-5 seconds') \
         LIMIT 1",
    )
    .bind(&user.id)
    .bind(room_id)
    .bind(body)
    .fetch_optional(&state.chat)
    .await?;
    if recent_match.is_some() {
        return Ok(StatusCode::NO_CONTENT);
    }

    db::drafts::upsert(&state.chat, &user.id, room_id, body).await?;
    // Light the sidebar indicator across the user's tabs. Idempotent: the
    // debounced PUT re-fires every ~1s of typing and re-swaps the same pencil
    // (a tiny per-user OOB), so no transition tracking is needed.
    broadcast_draft_changed(&state, &user.id, room_id, true);
    Ok(StatusCode::NO_CONTENT)
}
