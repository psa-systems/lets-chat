use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::bookmarks::{SavedListRow, SavedPage};
use crate::views::room::SingleMessageFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;
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

    // LC-178: refresh the viewer's /saved list in every tab.
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::SavedChanged {
            user_id: user.id.clone(),
        },
    );

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

    // LC-178: refresh the viewer's /saved list in every tab.
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::SavedChanged {
            user_id: user.id.clone(),
        },
    );

    render_bookmark_response(&state, &user, message_id, false).await
}

/// LC-479: upper bound on a bookmark label, enforced server-side (the input
/// also carries `maxlength`). Generous for short bucket names ("follow-up",
/// "read-later") while bounding the stored/rendered text.
const MAX_LABEL_CHARS: usize = 40;

#[derive(Deserialize)]
pub struct LabelForm {
    pub label: String,
}

/// POST /messages/{message_id}/bookmark/label
///
/// Set or clear the label ("folder") on the viewer's bookmark for this
/// message. An empty/whitespace value clears it back to unlabeled. No-op if
/// the message is not bookmarked by the viewer. Broadcasts `SavedChanged` so
/// every /saved tab (including this one) re-renders its list + filter chips
/// over the WebSocket; the request itself returns 204 (the form is
/// `hx-swap="none"`).
pub async fn post_label(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    Form(form): Form<LabelForm>,
) -> Result<Response, AppError> {
    let room_id = room_id_for_message(&state, message_id).await?;
    require_room_access(&state, &user, room_id).await?;

    let trimmed = form.label.trim();
    let capped: String = trimmed.chars().take(MAX_LABEL_CHARS).collect();
    let label = if capped.is_empty() {
        None
    } else {
        Some(capped.as_str())
    };
    db::bookmarks::set_label(&state.chat, &user.id, message_id, label).await?;

    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::SavedChanged {
            user_id: user.id.clone(),
        },
    );

    Ok(StatusCode::NO_CONTENT.into_response())
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

/// Resolve the viewer's bookmarks into render-ready `SavedListRow`s
/// (newest-first), with author labels resolved in one bulk lookup and a
/// per-row context link. Shared by `get_saved` (full page) and the LC-178 WS
/// re-render so both produce an identical list.
pub(crate) async fn build_saved_rows(
    state: &AppState,
    user: &User,
) -> Result<Vec<SavedListRow>, AppError> {
    let rows = db::bookmarks::bookmarks_for_user(&state.chat, &user.id).await?;

    // LC-684: resolve each DM row's peer up front so the peer id can join the
    // author ids in ONE bulk display-name lookup. Pre-LC-684 the peer id was
    // never resolved, so the DM caption fell back to the raw UUID.
    let mut peer_by_room: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for r in &rows {
        if r.room_type == "dm" && !peer_by_room.contains_key(&r.room_id) {
            if let Some(peer_id) = db::chat::get_dm_peer(&state.chat, r.room_id, &user.id).await? {
                peer_by_room.insert(r.room_id, peer_id);
            }
        }
    }

    // Resolve author + DM-peer labels in a single bulk auth lookup.
    let mut id_set: std::collections::HashSet<&str> =
        rows.iter().map(|r| r.author_user_id.as_str()).collect();
    for peer_id in peer_by_room.values() {
        id_set.insert(peer_id.as_str());
    }
    let ids: Vec<&str> = id_set.into_iter().collect();
    let labels = db::auth::display_names_for_ids(&state.auth, &ids).await?;
    // Display name if set, else `@username`, else the raw id (synthetic actor).
    let label_for = |id: &str| -> String {
        labels
            .get(id)
            .map(|(uname, dname)| match dname.as_deref() {
                Some(n) if !n.trim().is_empty() => n.to_string(),
                _ => format!("@{uname}"),
            })
            .unwrap_or_else(|| id.to_string())
    };

    // LC-684: bulk-load every saved message's attachments in one query, keyed by
    // message id, so a media saved message renders its real content instead of a
    // blank row.
    let message_ids: Vec<i64> = rows.iter().map(|r| r.message_id).collect();
    let mut attachments = db::uploads::attachments_for_messages(&state.chat, &message_ids).await?;

    // LC-684: the context link points at the permalink route (/m/{id}), which
    // redirects to /room/{id}#msg-{id} or /dm/{peer}#msg-{id}, so clicking a
    // saved message jumps to it in its original context. The caption still names
    // the room or DM peer.
    let mut entries: Vec<SavedListRow> = Vec::with_capacity(rows.len());
    for r in rows {
        let author_label = label_for(&r.author_user_id);

        let context_label = if r.room_type == "dm" {
            match peer_by_room.get(&r.room_id) {
                Some(peer_id) => format!("DM with {}", label_for(peer_id)),
                None => "Direct message".to_string(),
            }
        } else {
            format!("#{}", r.room_name)
        };
        let context_path = format!("/m/{}", r.message_id);

        // Same markdown pipeline as the timeline (sanitized). Empty in -> empty
        // out, which `has_content()` treats as "no text".
        let body_html = crate::views::markdown::render(&r.message_body, &[], &[]);

        entries.push(SavedListRow {
            message_id: r.message_id,
            author_label,
            body_html,
            attachments: attachments.remove(&r.message_id).unwrap_or_default(),
            message_created_at: r.message_created_at,
            saved_at: r.created_at,
            context_label,
            context_path,
            label: r.label,
        });
    }
    Ok(entries)
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
    let entries = build_saved_rows(&state, &user).await?;

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
