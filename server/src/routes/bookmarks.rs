use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::bookmarks::{SavedListRow, SavedPage};
use crate::views::room::SingleMessageFragment;
use crate::views::{html, Html};
use askama::Template;

/// Resolve the room id for the URL-supplied message id, or 404 for
/// nonexistent / soft-deleted messages. Shared by POST/DELETE.
async fn room_id_for_message(state: &AppState, message_id: i64) -> Result<i64, AppError> {
    db::bookmarks::room_for_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Caller must be able to see the room before they can bookmark a message
/// inside it. Uses the same accessibility check that gates page reads,
/// which already covers public rooms, private-room membership, and DM
/// participation.
async fn require_room_access(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// POST /messages/{message_id}/bookmark
///
/// Save the message to the viewer's private list. Idempotent. Returns the
/// re-rendered message bubble so the hover menu flips Save -> Unsave via
/// `hx-target=#msg-{id}` outerHTML swap. No WS broadcast: bookmarks are
/// private to the saving user.
pub async fn post_bookmark(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let room_id = room_id_for_message(&state, message_id).await?;
    require_room_access(&state, &user, room_id).await?;

    db::bookmarks::bookmark_message(&state.chat, &user.id, message_id).await?;

    render_bookmark_response(&state, &user, message_id, true).await
}

/// DELETE /messages/{message_id}/bookmark
///
/// Remove the bookmark. Idempotent.
pub async fn delete_bookmark(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let room_id = room_id_for_message(&state, message_id).await?;
    require_room_access(&state, &user, room_id).await?;

    db::bookmarks::unbookmark_message(&state.chat, &user.id, message_id).await?;

    render_bookmark_response(&state, &user, message_id, false).await
}

/// Re-render the single message bubble for the acting viewer so the hover
/// menu flips Save/Unsave. Mirrors `routes::pinned::render_pin_response`
/// minus the OOB strip (bookmarks have no per-room strip).
async fn render_bookmark_response(
    state: &AppState,
    user: &User,
    message_id: i64,
    is_bookmarked: bool,
) -> Result<Response, AppError> {
    let pinned = db::pinned::pinned_message_ids_for_room(
        &state.chat,
        db::bookmarks::room_for_message(&state.chat, message_id)
            .await?
            .ok_or(AppError::NotFound)?,
    )
    .await?;
    let view = super::load_message_view_for_viewer(
        state,
        user,
        message_id,
        pinned.contains(&message_id),
        is_bookmarked,
    )
    .await?;
    let bubble = SingleMessageFragment {
        message: &view,
        oob: false,
    }
    .render()?;
    Ok(Html(bubble).into_response())
}

/// GET /saved
///
/// Standalone page listing every bookmark the viewer has, newest-first.
/// Each row carries a link back to its room (or DM) so the viewer can
/// jump to the message in context.
pub async fn get_saved(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let rows = db::bookmarks::bookmarks_for_user(&state.chat, &user.id).await?;

    // Resolve author labels in a single bulk auth lookup.
    let author_ids: Vec<&str> = rows
        .iter()
        .map(|r| r.author_user_id.as_str())
        .collect::<std::collections::HashSet<&str>>()
        .into_iter()
        .collect();
    let labels = db::auth::display_names_for_ids(&state.auth, &author_ids).await?;

    // Build the per-row context_path: rooms use /room/{id}; DMs resolve
    // the peer via the room membership and use /dm/{peer}. DMs that no
    // longer have a resolvable peer (the peer was deleted) fall back to
    // /room/{id} so the link is never empty.
    let mut entries: Vec<SavedListRow> = Vec::with_capacity(rows.len());
    for r in rows {
        let author_label = labels
            .get(&r.author_user_id)
            .map(|(uname, dname)| match dname.as_deref() {
                Some(n) if !n.trim().is_empty() => n.to_string(),
                _ => uname.clone(),
            })
            .unwrap_or_else(|| r.author_user_id.clone());

        let (context_label, context_path) = if r.room_type == "dm" {
            match db::chat::get_dm_peer(&state.chat, r.room_id, &user.id).await? {
                Some(peer_id) => {
                    let peer_label = labels
                        .get(&peer_id)
                        .map(|(uname, dname)| match dname.as_deref() {
                            Some(n) if !n.trim().is_empty() => n.to_string(),
                            _ => uname.clone(),
                        })
                        .unwrap_or_else(|| peer_id.clone());
                    (format!("DM with @{peer_label}"), format!("/dm/{peer_id}"))
                }
                None => ("Direct message".to_string(), format!("/room/{}", r.room_id)),
            }
        } else {
            (format!("#{}", r.room_name), format!("/room/{}", r.room_id))
        };

        entries.push(SavedListRow {
            message_id: r.message_id,
            author_label,
            body: r.message_body,
            message_created_at: r.message_created_at,
            saved_at: r.created_at,
            context_label,
            context_path,
        });
    }

    // Sidebar + enclave switcher in one call; passing `None` for the
    // current enclave matches the chrome shown on the Home and Settings
    // pages, which is the right framing for a personal-list page.
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;

    let page = SavedPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        entries: &entries,
        asset_version: &state.asset_version,
    };
    html(&page)
}
