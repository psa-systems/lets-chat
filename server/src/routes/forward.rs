//! LC-278: message forwarding. A hover-menu "Forward" action opens a picker of
//! the viewer's post-able rooms + DMs; choosing one reposts the message there
//! (text + "Forwarded from <author>" attribution) as the viewer, appearing
//! live. v1 carries the body + attribution only (not attachments).

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::forward::{ForwardConfirm, ForwardDest, ForwardPicker};
use crate::views::{html, Html};

/// `display_name` if non-empty, else `username`.
fn label_for(rec: &crate::models::user::UserRecord) -> String {
    match rec.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => rec.username.clone(),
    }
}

/// `GET /messages/{message_id}/forward`
///
/// Render the destination picker: every non-DM room the viewer can access
/// (minus the source room) plus their DM conversations (minus blocked peers and
/// the source). Access to the SOURCE message is gated first. The POST re-checks
/// every gate, so the picker list is a convenience, not the security boundary.
pub async fn get_forward_picker(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    let src = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !db::chat::is_room_accessible(&state.chat, src.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let rooms: Vec<ForwardDest> = db::chat::list_rooms(&state.chat, &user.id, is_admin)
        .await?
        .into_iter()
        .filter(|r| r.id != src.room_id)
        .map(|r| ForwardDest {
            name: r.name.to_lowercase(),
            room_id: r.id,
            glyph: "#".into(),
            label: r.name,
        })
        .collect();

    let mut dms: Vec<ForwardDest> = Vec::new();
    for (room, peer_id) in db::chat::list_user_dm_rooms(&state.chat, &user.id).await? {
        if room.id == src.room_id {
            continue;
        }
        if db::auth::is_blocked_either_way(&state.auth, &user.id, &peer_id).await? {
            continue;
        }
        if let Some(rec) = db::auth::find_user_by_id(&state.auth, &peer_id).await? {
            let label = label_for(&rec);
            let name = format!("{} {}", label.to_lowercase(), rec.username.to_lowercase());
            dms.push(ForwardDest {
                room_id: room.id,
                glyph: "@".into(),
                label,
                name,
            });
        }
    }

    html(&ForwardPicker {
        message_id,
        rooms,
        dms,
    })
}

/// `POST /messages/{message_id}/forward/{dest_room_id}`
///
/// Repost the source message into `dest_room_id` as the viewer, prefixed with a
/// "Forwarded from <author>" attribution. Gated end to end: read access on the
/// source; banned/muted, access, posting-policy, and (for DM destinations) a
/// block check on the destination. Reuses `finalize_message_send` so the
/// forwarded message broadcasts live exactly like a normal post.
pub async fn post_forward(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((message_id, dest_room_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";

    let src = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !db::chat::is_room_accessible(&state.chat, src.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    if user.is_banned || user.mute_in_effect() {
        return Err(AppError::Forbidden);
    }

    let dest = db::chat::get_room(&state.chat, dest_room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !db::chat::is_room_accessible(&state.chat, dest.id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    if !super::room::can_post_with_policy(&state, &user, dest.id, &dest.posting_allowed_for).await?
    {
        return Err(AppError::Forbidden);
    }

    // For a DM destination, resolve the peer and refuse if blocked either way.
    let mut dest_label = format!("#{}", dest.name);
    if dest.room_type == "dm" {
        let dms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
        let Some((_, peer_id)) = dms.into_iter().find(|(r, _)| r.id == dest.id) else {
            // A DM the viewer is not a participant of.
            return Err(AppError::Forbidden);
        };
        if db::auth::is_blocked_either_way(&state.auth, &user.id, &peer_id).await? {
            return Err(AppError::Forbidden);
        }
        if let Some(rec) = db::auth::find_user_by_id(&state.auth, &peer_id).await? {
            dest_label = format!("@{}", label_for(&rec));
        }
    }

    // Build the forwarded body: a Markdown blockquote attribution + the original
    // body, each line quoted. Length-capped (LC-153: the render runs inline).
    let author_label = db::auth::find_user_by_id(&state.auth, &src.user_id)
        .await?
        .map(|r| label_for(&r))
        .unwrap_or_else(|| "(unknown)".to_string());
    let header = crate::i18n::translate_current("room-forward-attribution");
    let quoted: String = src.body.lines().map(|l| format!("> {l}\n")).collect();
    let mut body = format!("> **{header} {author_label}**\n>\n{quoted}");
    if body.chars().count() > super::room::MAX_MESSAGE_CHARS {
        body = body.chars().take(super::room::MAX_MESSAGE_CHARS).collect();
    }

    let new_id = db::chat::insert_message(&state.chat, dest.id, &user.id, &body).await?;
    super::room::finalize_message_send(&state, &dest, &user, new_id, &body, None).await?;

    html(&ForwardConfirm { dest_label })
}
