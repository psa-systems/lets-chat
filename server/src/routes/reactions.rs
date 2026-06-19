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

/// LC-274: minimal HTML-attribute escaping for values inlined into the raw
/// picker markup (there is no Askama template on this path to auto-escape).
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
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
    let reactor_titles = super::build_reactor_titles(&state, &counts).await;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .zip(reactor_titles)
        .map(|(r, title)| {
            ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &custom_emojis)
        })
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

    // LC-274: searchable keyword tokens for the unicode defaults so the filter
    // can find them by name (the glyph itself is not typeable). Custom emojis
    // are matched on their shortcode below.
    let unicode_defaults = [
        ("👍", "thumbsup +1 like"),
        ("❤", "heart love"),
        ("😂", "joy laugh lol"),
        ("🎉", "tada party celebrate"),
        ("😮", "wow open_mouth surprised"),
        ("😢", "cry sad"),
    ];
    let mut buttons = String::new();
    for (e, name) in &unicode_defaults {
        let encoded = percent_encoding::utf8_percent_encode(e, percent_encoding::NON_ALPHANUMERIC);
        buttons.push_str(&format!(
            r##"<button data-lc-emoji-name="{name}" hx-post="/messages/{message_id}/reactions/{encoded}" hx-target="#reactions-{message_id}" hx-swap="outerHTML" class="text-base">{e}</button>"##,
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
            r##"<button data-lc-emoji-name="{code}" hx-post="/messages/{id}/reactions/{enc}" hx-target="#reactions-{id}" hx-swap="outerHTML" class="text-base" title=":{code}:"><img class="h-5 w-5 inline-block align-text-bottom" src="/api/emojis/{eid}" alt=":{code}:"></button>"##,
            id = message_id,
            enc = encoded,
            code = emoji.shortcode,
            eid = emoji.id,
        ));
    }

    // LC-274: localized filter placeholder, attribute-escaped (translations are
    // operator/maintainer-controlled, but escape defensively so a future string
    // with a quote cannot break out of the attribute).
    let filter_label = attr_escape(&crate::i18n::translate_current("partials-reaction-filter"));
    // LC-288: empty recent-emoji row; the layout IIFE fills it from localStorage
    // when the picker opens (hidden when there is no history).
    let recent_label = attr_escape(&crate::i18n::translate_current("partials-reaction-recent"));
    let body = format!(
        r##"<div id="picker-{message_id}" class="inline-flex flex-col gap-1 rounded border border-border bg-surface-elevated p-1 shadow">
  <div class="flex items-center gap-1">
    <input data-lc-emoji-filter type="text" autocomplete="off" autofocus aria-label="{filter_label}" placeholder="{filter_label}" class="w-32 rounded border border-border px-1 py-0.5 text-xs">
    <button hx-get="/messages/{message_id}/reactions/cancel" hx-target="#picker-{message_id}" hx-swap="outerHTML" class="text-xs text-content-muted hover:text-content px-1" aria-label="Close">×</button>
  </div>
  <div data-lc-emoji-recent data-lc-recent-label="{recent_label}" class="hidden flex flex-wrap gap-1 items-center w-56 text-xs text-content-muted"></div>
  <div data-lc-emoji-grid class="flex flex-wrap gap-1 max-h-40 overflow-y-auto w-56">{buttons}</div>
</div>"##,
    );
    Ok(axum::response::Html(body).into_response())
}

/// GET /messages/:message_id/reactions/cancel
/// Replace the picker with the add-reaction button again. LC-377: this mirrors
/// the smiley pill rendered by `partials/reaction_bar.html`. The picker replaced
/// the add button in place (`hx-target="this"`), so after a cancel the user has
/// just been interacting with this message's reactions; the button is rendered
/// visible (not hover-gated) until the next full re-render reapplies the gate.
pub async fn cancel_picker(Path(message_id): Path<i64>) -> Response {
    let add_label = attr_escape(&crate::i18n::translate_current("partials-reaction-add"));
    let body = format!(
        r##"<button hx-get="/messages/{message_id}/reactions/picker" hx-target="this" hx-swap="outerHTML" aria-label="{add_label}" data-lc-tip="{add_label}" data-lc-tip-pos="top" aria-haspopup="dialog" class="lc-react-add inline-flex items-center justify-center h-6 rounded-full border border-border bg-surface-sunken px-1.5 text-content-muted hover:bg-surface-elevated hover:text-content"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-4 w-4" aria-hidden="true"><path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM7 9a1 1 0 100-2 1 1 0 000 2zm7-1a1 1 0 11-2 0 1 1 0 012 0zm-.464 5.535a1 1 0 10-1.415-1.414 3 3 0 01-4.242 0 1 1 0 00-1.415 1.414 5 5 0 007.072 0z" clip-rule="evenodd"/></svg></button>"##,
    );
    axum::response::Html(body).into_response()
}
