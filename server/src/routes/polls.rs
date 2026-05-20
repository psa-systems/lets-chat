//! LC-66: poll creation + voting.
//!
//! - `POST /room/{room_id}/poll` creates a poll from the compose modal.
//! - `POST /poll/{message_id}/vote` records a vote and re-renders the block.
//! - The `/poll "Q" "A" "B"` slash command routes through [`parse_command`]
//!   + [`create_poll`], shared with the modal handler.
//!
//! A poll is anchored to a `messages` row (body = question); voting mirrors
//! reactions (toggle a row, broadcast a re-rendered fragment to the room).

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Form;
use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::room::{build_poll_view, PollBlockFragment};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// Bounds. Generous but finite so a poll cannot become a wall of text.
const MAX_OPTIONS: usize = 10;
const MIN_OPTIONS: usize = 2;
const MAX_QUESTION_LEN: usize = 300;
const MAX_OPTION_LEN: usize = 150;

#[derive(Deserialize)]
pub struct CreatePollForm {
    pub question: String,
    /// One option per line.
    pub options: String,
    #[serde(default)]
    pub allows_multi: Option<String>,
    #[serde(default)]
    pub anonymous: Option<String>,
    /// Minutes until the poll auto-closes. Absent / 0 = open-ended.
    #[serde(default)]
    pub closes_in: Option<i64>,
}

#[derive(Deserialize)]
pub struct VoteForm {
    pub option_id: i64,
}

/// Validate question + options, returning the cleaned option list.
fn validate(question: &str, options: &[String]) -> Result<Vec<String>, AppError> {
    let q = question.trim();
    if q.is_empty() {
        return Err(AppError::BadRequest("poll question is required".into()));
    }
    if q.chars().count() > MAX_QUESTION_LEN {
        return Err(AppError::BadRequest("poll question is too long".into()));
    }
    let cleaned: Vec<String> = options
        .iter()
        .map(|o| o.trim().to_string())
        .filter(|o| !o.is_empty())
        .collect();
    if cleaned.len() < MIN_OPTIONS {
        return Err(AppError::BadRequest(
            "a poll needs at least two options".into(),
        ));
    }
    if cleaned.len() > MAX_OPTIONS {
        return Err(AppError::BadRequest(format!(
            "a poll allows at most {MAX_OPTIONS} options"
        )));
    }
    if cleaned.iter().any(|o| o.chars().count() > MAX_OPTION_LEN) {
        return Err(AppError::BadRequest("a poll option is too long".into()));
    }
    Ok(cleaned)
}

/// Shared creation path: insert the poll (atomic) and broadcast the message.
/// Used by both the modal handler and the slash command.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_poll(
    state: &AppState,
    room: &crate::models::Room,
    user: &User,
    question: &str,
    options: &[String],
    allows_multi: bool,
    anonymous: bool,
    closes_at: Option<String>,
) -> Result<(), AppError> {
    let options = validate(question, options)?;
    let message_id = db::polls::create(
        &state.chat,
        room.id,
        &user.id,
        question.trim(),
        &options,
        allows_multi,
        anonymous,
        closes_at.as_deref(),
    )
    .await?;
    super::room::finalize_message_send(state, room, user, message_id, question.trim()).await?;
    Ok(())
}

/// Parse a `/poll "Question" "Option A" "Option B" ...` command body (the
/// part after `/poll`). Returns `(question, options)`. Tokens are
/// double-quoted; everything outside quotes is ignored.
pub(crate) fn parse_command(rest: &str) -> Result<(String, Vec<String>), AppError> {
    let mut tokens = Vec::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut tok = String::new();
            for c2 in chars.by_ref() {
                if c2 == '"' {
                    break;
                }
                tok.push(c2);
            }
            tokens.push(tok);
        }
    }
    if tokens.len() < 1 + MIN_OPTIONS {
        return Err(AppError::BadRequest(
            "usage: /poll \"Question\" \"Option A\" \"Option B\" ...".into(),
        ));
    }
    let question = tokens.remove(0);
    Ok((question, tokens))
}

async fn require_post_access(
    state: &AppState,
    user: &User,
    room_id: i64,
) -> Result<crate::models::Room, AppError> {
    if user.is_banned || user.is_muted {
        return Err(AppError::Forbidden);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(room)
}

/// POST /room/{room_id}/poll  (compose modal)
pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<CreatePollForm>,
) -> Result<Response, AppError> {
    let room = require_post_access(&state, &user, room_id).await?;
    let options: Vec<String> = form.options.lines().map(|l| l.to_string()).collect();
    let closes_at = match form.closes_in {
        Some(m) if m > 0 => Some(
            (Utc::now() + Duration::minutes(m))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        ),
        _ => None,
    };
    create_poll(
        &state,
        &room,
        &user,
        &form.question,
        &options,
        form.allows_multi.is_some(),
        form.anonymous.is_some(),
        closes_at,
    )
    .await?;
    Ok((axum::http::StatusCode::OK, "").into_response())
}

/// POST /poll/{message_id}/vote
pub async fn post_vote(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    Form(form): Form<VoteForm>,
) -> Result<Html, AppError> {
    let poll = db::polls::get(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room_id = db::bookmarks::room_for_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    if db::polls::is_closed(&state.chat, message_id).await? {
        return Err(AppError::Conflict("this poll is closed".into()));
    }
    if !db::polls::option_belongs(&state.chat, message_id, form.option_id).await? {
        return Err(AppError::BadRequest("unknown poll option".into()));
    }

    if poll.allows_multi {
        // Toggle just this option.
        let mine = db::polls::user_votes(&state.chat, message_id, &user.id).await?;
        if mine.contains(&form.option_id) {
            db::polls::remove_vote(&state.chat, form.option_id, &user.id).await?;
        } else {
            db::polls::add_vote(&state.chat, form.option_id, &user.id).await?;
        }
    } else {
        // Single choice: clicking the current pick clears it; any other
        // pick replaces the existing vote.
        let mine = db::polls::user_votes(&state.chat, message_id, &user.id).await?;
        if mine == [form.option_id] {
            db::polls::remove_vote(&state.chat, form.option_id, &user.id).await?;
        } else {
            db::polls::clear_user_votes(&state.chat, message_id, &user.id).await?;
            db::polls::add_vote(&state.chat, form.option_id, &user.id).await?;
        }
    }

    state.hub.broadcast_to_room(
        room_id,
        &ChatEvent::PollUpdated {
            message_id,
            room_id,
        },
    );

    let view = build_poll_view(&state.chat, &state.auth, message_id, &user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    html(&PollBlockFragment { poll: &view })
}
