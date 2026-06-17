//! LC-323: the composer's `#channel` autocomplete endpoint.
//! `GET /rooms/{id}/channel-complete?q=` returns a listbox of rooms in the
//! room's enclave that the caller can access, matching the prefix. Mirrors
//! `routes::emoji_complete`: access-gated to the room, prefix beats substring,
//! capped. Returns a `#lc-channel-popover` fragment.
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::channel_complete::ChannelPopoverFragment;
use crate::views::{html, Html};

const MAX: usize = 8;
/// Bound the query so a pathological prefix cannot drive a huge scan. Room
/// names cap well under this; 64 matches the linkable-name limit.
const MAX_Q: usize = 64;

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    #[serde(default)]
    pub q: String,
}

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

    // Char-based truncation so a multibyte query never splits on a non-char
    // boundary.
    let q_lower: String = q.trim().to_ascii_lowercase().chars().take(MAX_Q).collect();

    let mut refs = super::channel_refs_for_room(&state, room_id, &viewer).await?;
    refs.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });

    // Prefix matches rank above substring matches; an empty query lists the
    // first MAX rooms alphabetically.
    let mut prefix = Vec::new();
    let mut other = Vec::new();
    for r in refs {
        let nl = r.name.to_ascii_lowercase();
        if q_lower.is_empty() || nl.starts_with(&q_lower) {
            prefix.push(r);
        } else if nl.contains(&q_lower) {
            other.push(r);
        }
    }
    let results: Vec<_> = prefix.into_iter().chain(other).take(MAX).collect();

    let frag = ChannelPopoverFragment { results: &results };
    html(&frag)
}
