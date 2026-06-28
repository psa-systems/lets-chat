//! LC-490: message acknowledgement / required-read tracking.
//!
//! - `POST /messages/{id}/ack` records the caller's acknowledgement (any room
//!   member) and returns the updated ack bar; broadcasts `Acknowledged`.
//! - `POST` / `DELETE /messages/{id}/ack-required` flip the requirement
//!   (author or room moderator only) and return the re-rendered bubble;
//!   broadcasts `AckRequiredChanged`.
//!
//! Mirrors `routes::reactions` (per-user roster + sub-region OOB) and
//! `routes::pinned` (author/mod boolean toggle + full-bubble OOB).

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html as AxumHtml, IntoResponse, Response};

use crate::auth::AuthUser;
use crate::db;
use crate::db::chat::RawMessage;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::room::{AckBarFragment, AckState, SingleMessageFragment};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// Caller must be able to see the message's room (mirror reactions/pinned).
async fn require_room_access(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Only the message author or a room moderator may toggle the ack requirement
/// (same gate as message delete: `MessageView::can_delete`).
async fn require_author_or_mod(
    state: &AppState,
    user: &User,
    m: &RawMessage,
) -> Result<(), AppError> {
    let ok = m.user_id == user.id
        || db::room_rbac::is_room_moderator(&state.chat, m.room_id, &user.id, &user.role).await?;
    if ok {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Re-render the whole bubble for the acting tab after a requirement toggle
/// (mirrors `pinned::render_pin_response` minus the pinned strip).
async fn render_bubble_response(
    state: &AppState,
    user: &User,
    m: &RawMessage,
) -> Result<Response, AppError> {
    let is_pinned = db::pinned::pinned_message_ids_for_room(&state.chat, m.room_id)
        .await?
        .contains(&m.id);
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &user.id, m.id).await?;
    let view =
        super::load_message_view_for_viewer(state, user, m.id, is_pinned, is_bookmarked).await?;
    let bubble = SingleMessageFragment {
        message: &view,
        oob: false,
    }
    .render()?;
    Ok(AxumHtml(bubble).into_response())
}

/// POST /messages/{message_id}/ack - record the caller's acknowledgement.
pub async fn post_ack(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_room_access(&state, &user, m.room_id).await?;
    if !db::acks::is_required(&state.chat, message_id).await? {
        return Err(AppError::BadRequest(
            "message does not require acknowledgement".into(),
        ));
    }
    db::acks::acknowledge(&state.chat, message_id, &user.id).await?;
    state.hub.broadcast_to_room(
        m.room_id,
        &ChatEvent::Acknowledged {
            room_id: m.room_id,
            message_id,
        },
    );
    let ack = super::build_ack_view(&state, message_id, &user.id)
        .await?
        .unwrap_or(AckState {
            count: 1,
            acked_by_me: true,
            ackers_title: String::new(),
        });
    html(&AckBarFragment { message_id, ack })
}

/// POST /messages/{message_id}/ack-required - flag a message as needing ack.
pub async fn post_require(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_room_access(&state, &user, m.room_id).await?;
    require_author_or_mod(&state, &user, &m).await?;
    db::acks::set_required(&state.chat, message_id, m.room_id, &user.id).await?;
    state.hub.broadcast_to_room(
        m.room_id,
        &ChatEvent::AckRequiredChanged {
            room_id: m.room_id,
            message_id,
        },
    );
    render_bubble_response(&state, &user, &m).await
}

/// DELETE /messages/{message_id}/ack-required - clear the requirement (and roster).
pub async fn delete_require(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_room_access(&state, &user, m.room_id).await?;
    require_author_or_mod(&state, &user, &m).await?;
    db::acks::clear_required(&state.chat, message_id).await?;
    state.hub.broadcast_to_room(
        m.room_id,
        &ChatEvent::AckRequiredChanged {
            room_id: m.room_id,
            message_id,
        },
    );
    render_bubble_response(&state, &user, &m).await
}
