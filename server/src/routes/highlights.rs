//! LC-529: reaction highlights recap. A per-room page listing the most-reacted
//! messages over a recent window. Non-LLM (a plain aggregate over
//! `message_reactions`), so it works with no AI configured. Mirrors the pins
//! page shape (`routes::pinned::get_room_pins`).

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::highlights::{HighlightRow, HighlightsPage};
use crate::views::{html, Html};

/// Recap window and size. Fixed for the MVP; the subheading text names the
/// window (`room-highlights-window`), so keep them in sync.
const WINDOW: &str = "-7 days";
const LIMIT: i64 = 10;
/// Per-row snippet length in chars, cut at a word boundary.
const SNIPPET_MAX_CHARS: usize = 240;

/// Collapse whitespace and truncate to `SNIPPET_MAX_CHARS`, breaking at the
/// last space when possible. Mirrors `routes::pinned::snippet_for` intent.
fn snippet_for(body: &str) -> String {
    let collapsed: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= SNIPPET_MAX_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(SNIPPET_MAX_CHARS).collect();
    let trimmed = match cut.rfind(' ') {
        Some(i) if i > SNIPPET_MAX_CHARS / 2 => &cut[..i],
        _ => &cut,
    };
    format!("{trimmed}...")
}

/// GET /room/{room_id}/highlights
pub async fn get_room_highlights(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "dm" {
        // DMs are a private 1:1 surface; a "highlights" recap is a channel idea.
        return Err(AppError::NotFound);
    }

    let highlights = db::highlights::top_reacted(&state.chat, room_id, WINDOW, LIMIT).await?;

    // One bulk auth lookup for every distinct author label.
    let mut unique: HashSet<&str> = HashSet::new();
    for h in &highlights {
        unique.insert(h.user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let names: HashMap<String, String> = db::auth::display_names_for_ids(&state.auth, &ids)
        .await?
        .into_iter()
        .map(|(id, (uname, dname))| {
            let label = match dname {
                Some(n) if !n.trim().is_empty() => n,
                _ => uname,
            };
            (id, label)
        })
        .collect();

    let rows: Vec<HighlightRow> = highlights
        .into_iter()
        .map(|h| HighlightRow {
            message_id: h.message_id,
            author_label: names
                .get(&h.user_id)
                .cloned()
                .unwrap_or_else(|| h.user_id.clone()),
            created_at: h.created_at,
            snippet: snippet_for(&h.body),
            total: h.total,
            emojis: h.emojis,
        })
        .collect();

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(
        &state,
        &user,
        super::enclave_for_room(&state, room_id).await?,
    )
    .await?;

    html(&HighlightsPage {
        user: &user,
        asset_version: &state.asset_version,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        room_label: format!("#{}", room.name),
        back_path: format!("/room/{room_id}"),
        rows,
    })
}
