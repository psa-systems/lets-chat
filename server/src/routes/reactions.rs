use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::room::{ReactionBarFragment, ReactionSurface, ReactionView};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// LC-595: `?surface=thread` on the reaction endpoints. Absent (the timeline,
/// and every pre-LC-595 caller) deserializes to the default, so existing URLs
/// keep working untouched.
#[derive(Debug, Default, serde::Deserialize)]
pub struct SurfaceQuery {
    #[serde(default)]
    pub surface: ReactionSurface,
}

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

/// Number of frequent emoji to return. A handful more than the quick-react
/// bar shows (QCAP=5 in layout.html) so the client can dedupe against recents
/// and still have a full row.
const FREQUENT_LIMIT: i64 = 8;

/// GET /api/reactions/frequent
/// LC-477: the caller's most-used Unicode reaction emoji as a JSON array,
/// most-frequent first. The quick-react bar (LC-302) fetches this once and uses
/// it as the primary seed so the one-tap row reflects the user's real habits
/// (cross-device, frequency-ranked) rather than only device-local MRU. No room
/// scope: the result is the caller's own aggregate across every room they have
/// reacted in, so no per-room access check applies.
pub async fn get_frequent(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Json<Vec<String>>, AppError> {
    let emojis = db::chat::top_reaction_emojis(&state.chat, &user.id, FREQUENT_LIMIT).await?;
    Ok(Json(emojis))
}

/// POST /messages/:message_id/reactions/:emoji
/// Toggle the caller's reaction for the given emoji on the given message.
/// Broadcasts a ReactionAdded or ReactionRemoved event to subscribers and
/// returns the updated reaction-bar fragment for the caller's tab.
pub async fn toggle_reaction(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((message_id, emoji)): Path<(i64, String)>,
    Query(SurfaceQuery { surface }): Query<SurfaceQuery>,
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

        // LC-495: run reaction_added workflow automations off the request path
        // (additions only). A matched rule posts as the automation bot; bot
        // reactors are ignored and the engine never re-enters itself.
        let st = state.clone();
        let reactor = user.clone();
        let emoji_owned = emoji.clone();
        let rid = m.room_id;
        tokio::spawn(async move {
            crate::automations::on_reaction_added(&st, rid, &reactor, &emoji_owned).await;
        });
    }

    let custom_emojis =
        db::custom_emojis::refs_for_room_and_user(&state.chat, m.room_id, &user.id).await?;
    let counts = db::chat::list_reactions(&state.chat, message_id, &user.id).await?;
    let reactor_titles = super::build_reactor_titles(&state, &counts).await;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .zip(reactor_titles)
        .map(|(r, title)| {
            ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &custom_emojis)
        })
        .collect();
    // LC-595: answer in the CALLER's id namespace. A chip clicked in the thread
    // panel must be replaced by markup carrying the thread's id; responding with
    // the timeline's would recreate the duplicate id LC-553 removed.
    let fragment = ReactionBarFragment {
        message_id,
        reactions: &reactions,
        surface,
        oob: false,
    };
    html(&fragment)
}

/// LC-389: build one react button for a Unicode emoji. The markup is byte-for-
/// byte the LC-384 shape (same `data-lc-emoji-name` filter hook, same
/// `hx-post`/`hx-target`/`hx-swap` the LC-288 recorder + close-on-react JS key
/// on), so expanding the set needed no JS change. `name` is the search keyword
/// string (human name + every shortcode), attribute-escaped.
fn push_unicode_button(
    buf: &mut String,
    message_id: i64,
    glyph: &str,
    name: &str,
    surface: ReactionSurface,
) {
    let encoded = percent_encoding::utf8_percent_encode(glyph, percent_encoding::NON_ALPHANUMERIC);
    // LC-595: the picker is opened FROM a surface and reacts back INTO it, so
    // every button carries that surface in both its URL and its target.
    let prefix = surface.id_prefix();
    let suffix = surface.url_suffix();
    buf.push_str(&format!(
        r##"<button type="button" data-lc-emoji-name="{name}" hx-post="/messages/{message_id}/reactions/{encoded}{suffix}" hx-target="#{prefix}reactions-{message_id}" hx-swap="outerHTML" class="lc-emoji-cell">{glyph}</button>"##,
    ));
}

/// GET /messages/:message_id/reactions/picker
/// Return the floating emoji picker. LC-389: the full Unicode set organized
/// into the eight standard categories (browsable via the tab strip + scroll)
/// plus the message's enclave custom emojis. The LC-274 filter searches name +
/// shortcodes across the whole set; the LC-288 recent row sits on top. DMs and
/// non-enclave rooms see the Unicode set without a Custom section.
pub async fn get_picker(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    Query(SurfaceQuery { surface }): Query<SurfaceQuery>,
) -> Result<Response, AppError> {
    // LC-149: gate before disclosing the room's custom-emoji inventory (and
    // before the handler doubles as an unauthenticated message-existence
    // oracle). 404 the message first so a non-member cannot distinguish
    // "no such message" from "no access".
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_room_access(&state, &user, m.room_id).await?;

    // LC-389: render the full set once per open. ~1900 buttons; reserve so the
    // build is a single allocation rather than a growth cascade.
    let mut tabs = String::new();
    let mut sections = String::with_capacity(400 * 1024);
    for (group, slug, key, tab_glyph) in crate::emoji_catalog::GROUPS {
        let label = attr_escape(&crate::i18n::translate_current(key));
        tabs.push_str(&format!(
            r##"<button type="button" data-lc-emoji-tab="{slug}" aria-label="{label}" title="{label}" class="lc-emoji-tab">{tab_glyph}</button>"##,
        ));
        sections.push_str(&format!(
            r##"<section class="lc-emoji-section" data-lc-emoji-cat="{slug}"><div class="lc-emoji-cat-label">{label}</div><div class="lc-emoji-cat-grid">"##,
        ));
        for emoji in group.emojis() {
            // Keyword string (name + every shortcode) is shared with the
            // composer picker, attribute-escaped against a stray quote.
            let name = attr_escape(&crate::emoji_catalog::keywords(emoji));
            push_unicode_button(&mut sections, message_id, emoji.as_str(), &name, surface);
        }
        sections.push_str("</div></section>");
    }

    // LC-389: per-enclave custom emojis as their own trailing section + tab.
    // Message-specific image buttons, unchanged from LC-384 (the LC-288 recorder
    // deliberately skips `:shortcode:` reactions, so these stay out of Recent).
    let custom =
        db::custom_emojis::refs_for_room_and_user(&state.chat, m.room_id, &user.id).await?;
    if !custom.is_empty() {
        let label = attr_escape(&crate::i18n::translate_current(
            "partials-reaction-cat-custom",
        ));
        tabs.push_str(&format!(
            r##"<button type="button" data-lc-emoji-tab="custom" aria-label="{label}" title="{label}" class="lc-emoji-tab">⭐</button>"##,
        ));
        sections.push_str(&format!(
            r##"<section class="lc-emoji-section" data-lc-emoji-cat="custom"><div class="lc-emoji-cat-label">{label}</div><div class="lc-emoji-cat-grid">"##,
        ));
        for emoji in &custom {
            let key = format!(":{}:", emoji.shortcode);
            let encoded =
                percent_encoding::utf8_percent_encode(&key, percent_encoding::NON_ALPHANUMERIC);
            // shortcode charset is [a-z0-9_]{2,32}, validated on insert,
            // so it is safe to inline into the markup without escaping.
            sections.push_str(&format!(
                r##"<button type="button" data-lc-emoji-name="{code}" hx-post="/messages/{id}/reactions/{enc}{suffix}" hx-target="#{prefix}reactions-{id}" hx-swap="outerHTML" class="lc-emoji-cell" title=":{code}:"><img class="h-5 w-5 inline-block align-text-bottom" src="/api/emojis/{eid}" alt=":{code}:"></button>"##,
                id = message_id,
                enc = encoded,
                prefix = surface.id_prefix(),
                suffix = surface.url_suffix(),
                code = emoji.shortcode,
                eid = emoji.id,
            ));
        }
        sections.push_str("</div></section>");
    }

    // LC-274: localized filter placeholder, attribute-escaped (translations are
    // operator/maintainer-controlled, but escape defensively so a future string
    // with a quote cannot break out of the attribute).
    let filter_label = attr_escape(&crate::i18n::translate_current("partials-reaction-filter"));
    // LC-288: empty recent-emoji row; the layout IIFE fills it from localStorage
    // when the picker opens (hidden when there is no history).
    let recent_label = attr_escape(&crate::i18n::translate_current("partials-reaction-recent"));
    // LC-384: the picker is a floating card rendered into the fixed-position
    // #lc-reaction-popover host (positioned by the layout.html LC-384 script).
    // The id MUST stay `picker-{id}` - the LC-288 recent-row JS derives the
    // message id from it and the LC-274 filter keys on `[id^="picker-"]`. The
    // close `x` dismisses the host client-side (data-lc-reaction-close).
    let close_label = attr_escape(&crate::i18n::translate_current("reminders-close"));
    let body = format!(
        r##"<div id="picker-{message_id}" class="w-72 rounded-lg border border-border bg-surface-elevated shadow-lg overflow-hidden">
  <div class="flex items-center gap-1 p-2 border-b border-border">
    <input data-lc-emoji-filter type="text" autocomplete="off" autofocus aria-label="{filter_label}" placeholder="{filter_label}" class="flex-1 min-w-0 rounded-md border border-border bg-surface-sunken px-2 py-1 text-xs focus:outline-none focus:border-accent focus:ring-1 focus:ring-accent">
    <button type="button" data-lc-reaction-close aria-label="{close_label}" class="shrink-0 inline-flex items-center justify-center h-6 w-6 rounded-md text-content-muted hover:bg-surface-sunken hover:text-content">&times;</button>
  </div>
  <div data-lc-emoji-recent data-lc-recent-label="{recent_label}" class="hidden flex flex-wrap items-center gap-1 px-2 py-1.5 text-xs text-content-muted border-b border-border"></div>
  <div data-lc-emoji-tabs class="lc-emoji-tabs">{tabs}</div>
  <div data-lc-emoji-grid class="lc-emoji-scroll">{sections}</div>
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
