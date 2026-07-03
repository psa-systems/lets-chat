//! LC-527: follow-up task lists. A list is created from a call transcript's
//! "## Action items" (see `routes::transcripts::create_followups`) and posted to
//! the room as a message-anchored checklist (mirroring polls, LC-66). These
//! handlers cover the interactive part: toggling an item done and self-claiming
//! it. Both mutate a row, broadcast a re-rendered fragment to the room, and
//! return the block to the acting user (mirrors `routes::polls::post_vote`).

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::room::{build_follow_up_view, FollowUpBlockFragment};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// Parse the "## Action items" bullet list out of a transcript summary's
/// markdown. Returns one cleaned string per item. Skips a bare "None."
/// placeholder and bounds both the item count and each item's length so a
/// runaway model response cannot create an unbounded checklist.
pub(crate) fn parse_action_items(summary_md: &str) -> Vec<String> {
    const MAX_ITEMS: usize = 50;
    const MAX_LEN: usize = 500;
    let mut items = Vec::new();
    let mut in_section = false;
    for line in summary_md.lines() {
        let t = line.trim();
        // A heading line switches sections: enter on "Action items", leave on
        // any other heading.
        if let Some(rest) = t.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim().to_lowercase();
            in_section = title.starts_with("action item");
            continue;
        }
        if !in_section {
            continue;
        }
        let Some(mut item) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
        else {
            continue;
        };
        item = item.trim();
        // Strip a task-list checkbox marker if the model emitted one.
        for pref in ["[ ] ", "[x] ", "[X] "] {
            if let Some(r) = item.strip_prefix(pref) {
                item = r.trim();
                break;
            }
        }
        if item.is_empty() {
            continue;
        }
        if item
            .trim_end_matches('.')
            .trim()
            .eq_ignore_ascii_case("none")
        {
            continue;
        }
        let cleaned: String = item.chars().take(MAX_LEN).collect();
        items.push(cleaned);
        if items.len() >= MAX_ITEMS {
            break;
        }
    }
    items
}

/// Resolve the item's room and gate the acting user on room access. Returns
/// `(message_id, room_id)`.
async fn require_item_access(
    state: &AppState,
    user: &crate::models::User,
    item_id: i64,
) -> Result<(i64, i64), AppError> {
    let item = db::followups::item(&state.chat, item_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room_id = db::bookmarks::room_for_message(&state.chat, item.message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok((item.message_id, room_id))
}

/// Broadcast the change to the room and return the freshly-rendered block to
/// the acting user (other viewers update over the WS).
async fn broadcast_and_render(
    state: &AppState,
    user: &crate::models::User,
    message_id: i64,
    room_id: i64,
) -> Result<Html, AppError> {
    state.hub.broadcast_to_room(
        room_id,
        &ChatEvent::FollowUpUpdated {
            message_id,
            room_id,
        },
    );
    let view = build_follow_up_view(&state.chat, &state.auth, message_id, &user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    html(&FollowUpBlockFragment { follow_up: &view })
}

/// POST /follow-up/{item_id}/toggle - mark an item done / not-done.
pub async fn post_toggle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(item_id): Path<i64>,
) -> Result<Html, AppError> {
    let (message_id, room_id) = require_item_access(&state, &user, item_id).await?;
    db::followups::toggle_done(&state.chat, item_id, &user.id).await?;
    broadcast_and_render(&state, &user, message_id, room_id).await
}

/// POST /follow-up/{item_id}/claim - self-claim / release an item.
pub async fn post_claim(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(item_id): Path<i64>,
) -> Result<Html, AppError> {
    let (message_id, room_id) = require_item_access(&state, &user, item_id).await?;
    db::followups::toggle_claim(&state.chat, item_id, &user.id).await?;
    broadcast_and_render(&state, &user, message_id, room_id).await
}

#[cfg(test)]
mod tests {
    use super::parse_action_items;

    #[test]
    fn extracts_bullets_from_action_items_section() {
        let md = "## Summary\nWe met.\n\n## Action items\n- Ship the release\n* Email Bob\n+ Update the docs\n";
        assert_eq!(
            parse_action_items(md),
            vec!["Ship the release", "Email Bob", "Update the docs"]
        );
    }

    #[test]
    fn ignores_summary_bullets_and_stops_at_next_heading() {
        let md =
            "## Summary\n- not a task\n## Action items\n- Real task\n## Notes\n- also not a task";
        assert_eq!(parse_action_items(md), vec!["Real task"]);
    }

    #[test]
    fn strips_checkbox_markers_and_skips_none() {
        assert_eq!(
            parse_action_items("## Action items\n- [ ] Do a thing\n- [x] Done thing"),
            vec!["Do a thing", "Done thing"]
        );
        assert!(parse_action_items("## Action items\nNone.").is_empty());
        assert!(parse_action_items("## Action items\n- None").is_empty());
    }

    #[test]
    fn no_section_yields_nothing() {
        assert!(parse_action_items("## Summary\nJust a summary, no items.").is_empty());
        assert!(parse_action_items("").is_empty());
    }

    #[test]
    fn case_insensitive_heading() {
        assert_eq!(
            parse_action_items("## ACTION ITEMS\n- Task one"),
            vec!["Task one"]
        );
    }
}
