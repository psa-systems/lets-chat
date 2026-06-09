//! LC-250: "mark all as read". One action clears the viewer's unread message
//! badges and the paired mention chips across every conversation they can see,
//! then returns the re-rendered sidebar (and refreshes their other tabs live).

use std::collections::HashSet;

use axum::extract::State;
use axum::http::HeaderMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::Html;
use crate::ws::events::ChatEvent;

/// `POST /read-all`
///
/// Advances the read watermark to the latest message in every room/DM the
/// viewer has unread in, and clears those rooms' unread mentions - mirroring
/// exactly what opening a single room does (`set_last_read` +
/// `mark_mentions_read_for_room`), so the bulk path and the open-one path
/// converge. Idempotent: `upsert_dm_read` keeps the max watermark, so a second
/// click is a no-op.
pub async fn post_read_all(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";

    // The union of rooms with unread messages (rooms + DMs) and rooms with
    // unread mentions. Each source already scopes to the viewer's visible set.
    let mut room_ids: HashSet<i64> = HashSet::new();
    for (room_id, _) in db::chat::list_room_unread_counts(&state.chat, &user.id, is_admin).await? {
        room_ids.insert(room_id);
    }
    for (room_id, _) in db::chat::list_dm_unread_counts(&state.chat, &user.id).await? {
        room_ids.insert(room_id);
    }
    for (room_id, _) in db::mentions::count_unread_mentions_per_room(&state.chat, &user.id).await? {
        room_ids.insert(room_id);
    }

    for room_id in room_ids {
        // Best-effort per room: a hiccup on one must not abort the batch and
        // leave the rest unread.
        let Ok(Some(mid)) = db::chat::latest_message_id(&state.chat, room_id).await else {
            continue;
        };
        let _ = db::chat::set_last_read(&state.chat, &user.id, room_id, mid).await;
        let _ =
            db::mentions::mark_mentions_read_for_room(&state.chat, &user.id, room_id, mid).await;
    }

    // Refresh the viewer's other tabs (the acting tab gets the response below).
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::ReadAllChanged {
            user_id: user.id.clone(),
        },
    );

    super::sidebar_categories::render_sidebar_fragment(&state, &user, &headers).await
}
