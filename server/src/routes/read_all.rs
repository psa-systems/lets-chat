//! LC-250: "mark all as read". One action clears the viewer's unread message
//! badges and the paired mention chips across every conversation they can see,
//! then returns the re-rendered sidebar (and refreshes their other tabs live).

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::http::HeaderMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::Html;
use crate::ws::events::ChatEvent;

/// Mark a single room/DM read for `user`: advance the watermark to the room's
/// latest message and clear its unread mentions, mirroring what opening the
/// room does. Shared by the bulk path and the per-room path. No-op (Ok) for an
/// empty room. `set_last_read` keeps the max watermark, so repeat calls are
/// idempotent.
async fn mark_room_read(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    if let Some(mid) = db::chat::latest_message_id(&state.chat, room_id).await? {
        db::chat::set_last_read(&state.chat, &user.id, room_id, mid).await?;
        db::mentions::mark_mentions_read_for_room(&state.chat, &user.id, room_id, mid).await?;
    }
    Ok(())
}

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
    for (room_id, _) in
        db::mentions::count_unread_mentions_per_room(&state.chat, &user.id, is_admin).await?
    {
        room_ids.insert(room_id);
    }

    for room_id in room_ids {
        // Best-effort per room: a hiccup on one must not abort the batch and
        // leave the rest unread.
        let _ = mark_room_read(&state, &user, room_id).await;
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

/// `POST /room/{room_id}/read`
///
/// LC-258: clear a SINGLE room/DM's unread badge (and its paired mention chips)
/// without opening it - the per-room companion to `/read-all`. Access-gated by
/// the same `is_room_accessible` check that guards page reads, so a viewer
/// cannot mark a room they cannot see. Reuses the LC-250 `ReadAllChanged`
/// broadcast (it just re-renders the viewer's sidebar) to refresh other tabs.
pub async fn post_room_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    headers: HeaderMap,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    mark_room_read(&state, &user, room_id).await?;

    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::ReadAllChanged {
            user_id: user.id.clone(),
        },
    );

    super::sidebar_categories::render_sidebar_fragment(&state, &user, &headers).await
}

/// `POST /messages/{message_id}/unread`
///
/// LC-286: mark a conversation unread from a message - the inverse of the read
/// paths. Rewinds the room's read watermark to just before the message
/// (`message_id - 1`), so `get_unread_count`'s `id > watermark` re-raises this
/// message and everything after it. Access-gated to the message's room. Reuses
/// the `ReadAllChanged` broadcast (sidebar re-render, live across tabs).
pub async fn post_message_unread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    headers: HeaderMap,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, m.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // Watermark just below the target so the target (and newer) become unread.
    let watermark = (message_id - 1).max(0);
    db::chat::rewind_dm_read(&state.chat, &user.id, m.room_id, watermark).await?;

    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::ReadAllChanged {
            user_id: user.id.clone(),
        },
    );

    super::sidebar_categories::render_sidebar_fragment(&state, &user, &headers).await
}
