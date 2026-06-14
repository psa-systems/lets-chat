use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::emoji_complete::{EmojiPopoverFragment, EmojiSuggestion};
use crate::views::{html, Html};

const MAX: usize = 8;
/// Bound the query so a pathological prefix cannot drive a huge scan. A real
/// shortcode is `[a-z0-9_+-]`-ish and short; 32 is generous headroom.
const MAX_Q: usize = 32;

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    #[serde(default)]
    pub q: String,
}

/// GET /rooms/:room_id/emoji-complete?q=
///
/// Returns a small `<ul role="listbox">` of emoji candidates for the composer's
/// `:shortcode:` autocomplete. Access-gated identically to `/users/mentions`:
/// the caller must be able to see `room_id` (the row also discloses the
/// enclave's custom-emoji inventory). Always returns 200 with an HTML body so
/// the composer's `htmx.ajax(...)` can swap directly into `#lc-emoji-popover`.
///
/// Order: the room enclave's custom emojis whose shortcode matches first (the
/// room-specific, higher-value set), then Unicode emojis. An empty `q` (a bare
/// `:`) returns the room's custom emojis only, so the list is never the full
/// Unicode dump.
pub async fn get_autocomplete(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Path(room_id): Path<i64>,
    Query(AutocompleteQuery { q }): Query<AutocompleteQuery>,
) -> Result<Html, AppError> {
    let is_admin = viewer.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &viewer.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let q_lower = {
        let t = q.trim().to_ascii_lowercase();
        if t.len() > MAX_Q {
            t[..MAX_Q].to_string()
        } else {
            t
        }
    };

    let mut results: Vec<EmojiSuggestion> = Vec::with_capacity(MAX);

    // Custom emojis (per-enclave) first. refs_for_room returns the room's
    // enclave set, or empty for DMs / non-enclave rooms. Prefix matches rank
    // above substring matches; an empty query lists them all (up to MAX).
    let mut custom = db::custom_emojis::refs_for_room(&state.chat, room_id).await?;
    custom.sort_by(|a, b| a.shortcode.cmp(&b.shortcode));
    let mut custom_prefix = Vec::new();
    let mut custom_other = Vec::new();
    for e in custom {
        let sc = e.shortcode.to_ascii_lowercase();
        if q_lower.is_empty() || sc.starts_with(&q_lower) {
            custom_prefix.push(e);
        } else if sc.contains(&q_lower) {
            custom_other.push(e);
        }
    }
    for e in custom_prefix.into_iter().chain(custom_other) {
        if results.len() >= MAX {
            break;
        }
        results.push(EmojiSuggestion::custom(e.shortcode, e.id));
    }

    // Unicode emojis fill the remainder, but only once the user has typed a
    // prefix (a bare `:` surfaces just the room's custom set, never 1800 rows).
    if !q_lower.is_empty() && results.len() < MAX {
        let mut uni_prefix = Vec::new();
        let mut uni_other = Vec::new();
        for emoji in emojis::iter() {
            // Match on any shortcode (prefix beats substring) or, failing that,
            // the human name as a substring. Keep the first shortcode as the
            // display label / insert-token source.
            let Some(primary) = emoji.shortcodes().next() else {
                continue;
            };
            let mut is_prefix = false;
            let mut is_match = false;
            for sc in emoji.shortcodes() {
                let scl = sc.to_ascii_lowercase();
                if scl.starts_with(&q_lower) {
                    is_prefix = true;
                    is_match = true;
                    break;
                }
                if scl.contains(&q_lower) {
                    is_match = true;
                }
            }
            if !is_match && emoji.name().to_ascii_lowercase().contains(&q_lower) {
                is_match = true;
            }
            if !is_match {
                continue;
            }
            let s = EmojiSuggestion::unicode(
                emoji.as_str(),
                primary.to_string(),
                emoji.name().to_string(),
            );
            if is_prefix {
                uni_prefix.push(s);
            } else {
                uni_other.push(s);
            }
        }
        for s in uni_prefix.into_iter().chain(uni_other) {
            if results.len() >= MAX {
                break;
            }
            results.push(s);
        }
    }

    let frag = EmojiPopoverFragment { results: &results };
    html(&frag)
}
