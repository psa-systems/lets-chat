use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::room::{ReactionBarFragment, ReactionView};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// POST /messages/:message_id/reactions/:emoji
/// Toggle the caller's reaction for the given emoji on the given message.
/// Broadcasts a ReactionAdded or ReactionRemoved event to subscribers and
/// returns the updated reaction-bar fragment for the caller's tab.
pub async fn toggle_reaction(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((message_id, emoji)): Path<(i64, String)>,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let added = db::chat::toggle_reaction(&state.chat, message_id, &user.id, &emoji).await?;
    let event = if added {
        ChatEvent::ReactionAdded {
            message_id,
            room_id: m.room_id,
            emoji: emoji.clone(),
            user_id: user.id.clone(),
        }
    } else {
        ChatEvent::ReactionRemoved {
            message_id,
            room_id: m.room_id,
            emoji: emoji.clone(),
            user_id: user.id.clone(),
        }
    };
    state.hub.broadcast_to_room(m.room_id, &event);

    let counts = db::chat::list_reactions(&state.chat, message_id, &user.id).await?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let fragment = ReactionBarFragment {
        message_id,
        reactions: &reactions,
    };
    html(&fragment)
}

/// GET /messages/:message_id/reactions/picker
/// Return an inline emoji picker that replaces the `+` button.
pub async fn get_picker(Path(message_id): Path<i64>) -> Response {
    let emojis = ["👍", "❤", "😂", "🎉", "😮", "😢"];
    let buttons: String = emojis
        .iter()
        .map(|e| {
            format!(
                r##"<button hx-post="/messages/{id}/reactions/{e}" hx-target="#reactions-{id}" hx-swap="outerHTML" class="text-base">{e}</button>"##,
                id = message_id,
                e = e
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let body = format!(
        r##"<div class="inline-flex gap-1">{buttons}<button hx-get="/messages/{id}/reactions/cancel" hx-target="this" hx-swap="outerHTML" class="text-xs text-slate-500">×</button></div>"##,
        id = message_id,
        buttons = buttons,
    );
    axum::response::Html(body).into_response()
}

/// GET /messages/:message_id/reactions/cancel
/// Replace the picker with the `+` button again.
pub async fn cancel_picker(Path(message_id): Path<i64>) -> Response {
    let body = format!(
        r##"<button hx-get="/messages/{id}/reactions/picker" hx-target="this" hx-swap="outerHTML" class="text-xs text-slate-500 hover:text-slate-700">+</button>"##,
        id = message_id,
    );
    axum::response::Html(body).into_response()
}
