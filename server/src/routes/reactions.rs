use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::room::{ReactionBarFragment, ReactionView};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

// percent_encoding is already a transitive dep via the workspace; the picker
// uses it to URL-encode shortcodes/glyphs into the reactions path segment.

/// LC-149: every reaction handler must verify the caller can access the
/// message's room before touching it, exactly like the pinned/bookmark/poll
/// handlers. Without this a caller could react in (or enumerate the custom
/// emoji of) a private room / DM / enclave they are not a member of.
async fn require_room_access(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Upper bound on a reaction token. Legitimate values are a single unicode
/// emoji (a few bytes) or a `:shortcode:` (shortcode is `[a-z0-9_]{2,32}`, so
/// <= 34 bytes). 64 bytes is generous headroom while blocking junk rows from
/// the raw `{emoji}` path segment (LC-149 / audit S3).
const MAX_REACTION_BYTES: usize = 64;

fn is_valid_reaction(emoji: &str) -> bool {
    !emoji.is_empty() && emoji.len() <= MAX_REACTION_BYTES && !emoji.chars().any(|c| c.is_control())
}

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
    require_room_access(&state, &user, m.room_id).await?;
    if !is_valid_reaction(&emoji) {
        return Err(AppError::BadRequest("invalid reaction".into()));
    }

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

    // LC-75: publish reaction.added to outgoing webhooks (additions only).
    // LC-78: actor describes the REACTOR (not the message author). v1 only
    // allows real users to react, so this is always kind="user"; the field
    // is included for symmetry with message.* events and so a future
    // bridge-reactions endpoint can swap it without changing the schema.
    if added {
        crate::outgoing::enqueue(
            &state.chat,
            "reaction.added",
            m.room_id,
            serde_json::json!({
                "message_id": message_id,
                "emoji": emoji,
                "user_id": user.id,
                "actor": super::outgoing_actor(&user.id, None, None, None, None),
            }),
        )
        .await;
    }

    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
    let counts = db::chat::list_reactions(&state.chat, message_id, &user.id).await?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView::new(r.emoji, r.count, r.reacted_by_me, &custom_emojis))
        .collect();
    let fragment = ReactionBarFragment {
        message_id,
        reactions: &reactions,
    };
    html(&fragment)
}

/// GET /messages/:message_id/reactions/picker
/// Return an inline emoji picker that replaces the `+` button. The picker
/// surfaces a small unicode default set plus every custom emoji defined in
/// the message's enclave (DMs and non-enclave rooms see unicode only).
pub async fn get_picker(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    // LC-149: gate before disclosing the room's custom-emoji inventory (and
    // before the handler doubles as an unauthenticated message-existence
    // oracle). 404 the message first so a non-member cannot distinguish
    // "no such message" from "no access".
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_room_access(&state, &user, m.room_id).await?;

    let unicode_defaults = ["👍", "❤", "😂", "🎉", "😮", "😢"];
    let mut buttons = String::new();
    for e in &unicode_defaults {
        let encoded = percent_encoding::utf8_percent_encode(e, percent_encoding::NON_ALPHANUMERIC);
        buttons.push_str(&format!(
            r##"<button hx-post="/messages/{message_id}/reactions/{encoded}" hx-target="#reactions-{message_id}" hx-swap="outerHTML" class="text-base">{e}</button>"##,
        ));
    }

    let emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
    for emoji in &emojis {
        let key = format!(":{}:", emoji.shortcode);
        let encoded =
            percent_encoding::utf8_percent_encode(&key, percent_encoding::NON_ALPHANUMERIC);
        // shortcode charset is [a-z0-9_]{2,32}, validated on insert,
        // so it is safe to inline into the markup without escaping.
        buttons.push_str(&format!(
            r##"<button hx-post="/messages/{id}/reactions/{enc}" hx-target="#reactions-{id}" hx-swap="outerHTML" class="text-base" title=":{code}:"><img class="h-5 w-5 inline-block align-text-bottom" src="/api/emojis/{eid}" alt=":{code}:"></button>"##,
            id = message_id,
            enc = encoded,
            code = emoji.shortcode,
            eid = emoji.id,
        ));
    }

    let body = format!(
        r##"<div id="picker-{message_id}" class="inline-flex gap-1 items-center">{buttons}<button hx-get="/messages/{message_id}/reactions/cancel" hx-target="#picker-{message_id}" hx-swap="outerHTML" class="text-xs text-slate-500">×</button></div>"##,
    );
    Ok(axum::response::Html(body).into_response())
}

/// GET /messages/:message_id/reactions/cancel
/// Replace the picker with the `+` button again.
pub async fn cancel_picker(Path(message_id): Path<i64>) -> Response {
    let body = format!(
        r##"<button hx-get="/messages/{message_id}/reactions/picker" hx-target="this" hx-swap="outerHTML" class="text-xs text-slate-500 hover:text-slate-700">+</button>"##,
    );
    axum::response::Html(body).into_response()
}
