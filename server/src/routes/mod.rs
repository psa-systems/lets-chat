use askama::Template;
use axum::extract::{OriginalUri, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::collections::HashMap;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::{enforce_2fa_enrollment, inject_user, OptionalUser};
use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;
use crate::version;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::not_found::NotFoundPage;
use crate::ws::events::ChatEvent;

#[cfg(feature = "standalone")]
mod admin;
mod auth;
mod avatar;
mod dm;
mod dm_mute;
mod enclave;
mod home;
mod mentions;
mod notify_prefs;
mod pinned;
mod push;
mod reactions;
mod room;
#[cfg(feature = "saas")]
mod saas_auth;
mod search;
mod settings;
mod status;
pub(crate) mod two_factor;
mod unfurl;
mod uploads;
mod users;
mod ws;

/// Override a persisted status with `"offline"` when the user has no live
/// WebSocket. Online presence is hub-derived; the `users.status` column only
/// stores the explicit user choice plus the idle auto-flip.
pub(crate) fn effective_status(state: &AppState, user_id: &str, persisted: &str) -> String {
    if state.hub.is_user_connected(user_id) {
        persisted.to_string()
    } else {
        "offline".to_string()
    }
}

/// Per-message author metadata cached by callers that build many MessageViews.
/// Keeps the join across auth.db and chat.db to a single field per author.
#[derive(Clone)]
pub(crate) struct AuthorMeta {
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
}

impl AuthorMeta {
    pub fn unknown() -> Self {
        Self {
            username: "(unknown)".to_string(),
            display_name: None,
            avatar_ext: None,
            status: db::auth::STATUS_ACTIVE.to_string(),
            custom_status: None,
        }
    }
}

impl From<crate::models::user::UserRecord> for AuthorMeta {
    fn from(r: crate::models::user::UserRecord) -> Self {
        Self {
            username: r.username,
            display_name: r.display_name,
            avatar_ext: r.avatar_ext,
            status: r.status,
            custom_status: r.custom_status,
        }
    }
}

pub(crate) async fn load_author_meta(
    state: &AppState,
    user_id: &str,
    viewer_id: &str,
) -> Result<AuthorMeta, AppError> {
    let mut meta = db::auth::find_user_by_id(&state.auth, user_id)
        .await?
        .map(AuthorMeta::from)
        .unwrap_or_else(AuthorMeta::unknown);
    // The viewer is by definition present (they are loading this page) even
    // before their WebSocket finishes opening, so trust their persisted
    // status rather than the hub's connection set.
    if user_id != viewer_id {
        meta.status = effective_status(state, user_id, &meta.status);
    }
    Ok(meta)
}

/// Build a complete `MessageView` for one message as seen by `viewer`.
/// Resolves author meta, reactions, follow-up grouping, reply count,
/// attachments, and mentions. Returns `AppError::NotFound` if the message
/// does not exist. `is_pinned` is passed in by the caller so handlers that
/// already know the post-mutation pin state (pin/unpin) don't pay an extra
/// query; callers that don't care can pass `false`.
pub(crate) async fn load_message_view_for_viewer(
    state: &AppState,
    viewer: &User,
    message_id: i64,
    is_pinned: bool,
) -> Result<crate::views::room::MessageView, AppError> {
    use crate::views::room::{MessageView, ReactionView};
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let meta = load_author_meta(state, &m.user_id, &viewer.id).await?;
    let can_edit = m.user_id == viewer.id;
    let can_delete = m.user_id == viewer.id || viewer.role == "admin" || viewer.role == "moderator";
    let reactions: Vec<ReactionView> = db::chat::list_reactions(&state.chat, m.id, &viewer.id)
        .await?
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id).await?;
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (m.user_id.as_str(), m.created_at.as_str()),
    );
    let reply_count = db::chat::count_replies(&state.chat, m.id).await?;
    let attachments = db::uploads::attachments_for_message(&state.chat, m.id).await?;
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[m.id])
        .await?
        .remove(&m.id)
        .unwrap_or_default();
    Ok(MessageView {
        id: m.id,
        room_id: m.room_id,
        user_id: m.user_id.clone(),
        username: meta.username,
        display_name: meta.display_name,
        avatar_ext: meta.avatar_ext,
        status: meta.status,
        custom_status: meta.custom_status,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
        reply_count,
        parent_id: m.parent_id,
        attachments,
        mentions,
        is_pinned,
    })
}

/// Refresh the caller's `last_active_at` and, if that bumped the row from
/// idle back to active, broadcast `UserStatusChanged` so subscribers can
/// update their UI. DND status is sticky and never flips here.
pub(crate) async fn touch_user_and_maybe_broadcast(state: &AppState, user_id: &str) {
    let flipped = match db::auth::touch_user_activity(&state.auth, user_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, user_id, "touch_user_activity failed");
            return;
        }
    };
    if !flipped {
        return;
    }
    if let Ok(Some(record)) = db::auth::find_user_by_id(&state.auth, user_id).await {
        state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
            user_id: record.id,
            status: record.status,
            custom_status: record.custom_status,
        });
    }
}

/// Build the sidebar's room and DM-peer view-models scoped to the caller's
/// current location. When `current_enclave` is `None` (Home), the sidebar
/// shows DMs only and the rooms list is empty. When `current_enclave` is
/// `Some(eid)`, the sidebar shows the enclave's rooms (filtered by the
/// caller's per-room access for private rooms) and the DM list is empty.
pub(crate) async fn load_sidebar(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<(Vec<SidebarRoom>, Vec<SidebarPeer>), AppError> {
    let is_admin = user.role == "admin";

    let (sidebar_rooms, sidebar_peers) = if let Some(eid) = current_enclave {
        let rooms = db::chat::list_rooms_in_enclave(&state.chat, eid, &user.id, is_admin).await?;
        let room_unreads: HashMap<i64, i64> =
            db::chat::list_room_unread_counts(&state.chat, &user.id, is_admin)
                .await?
                .into_iter()
                .collect();
        let mention_counts: HashMap<i64, i64> =
            db::mentions::count_unread_mentions_per_room(&state.chat, &user.id)
                .await?
                .into_iter()
                .collect();
        let mute_modes: HashMap<i64, db::notifications::MuteMode> =
            db::notifications::room_mute_modes_for_user(&state.chat, &user.id).await?;
        let sidebar_rooms: Vec<SidebarRoom> = rooms
            .into_iter()
            .map(|r| SidebarRoom {
                unread: *room_unreads.get(&r.id).unwrap_or(&0),
                mentions: *mention_counts.get(&r.id).unwrap_or(&0),
                mute_mode: mute_modes
                    .get(&r.id)
                    .copied()
                    .unwrap_or(db::notifications::MuteMode::None)
                    .as_str()
                    .to_string(),
                id: r.id,
                name: r.name,
                active: false,
            })
            .collect();
        (sidebar_rooms, Vec::new())
    } else {
        let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
        let dm_unreads_by_room: HashMap<i64, i64> =
            db::chat::list_dm_unread_counts(&state.chat, &user.id)
                .await?
                .into_iter()
                .collect();
        let dm_mute_modes: HashMap<i64, db::notifications::MuteMode> =
            db::notifications::room_mute_modes_for_user(&state.chat, &user.id).await?;
        let mut sidebar_peers: Vec<SidebarPeer> = Vec::with_capacity(dm_rooms.len());
        for (room, peer_id) in &dm_rooms {
            // Skip DMs with users who are blocked in either direction. The
            // peer record may still exist; we just don't surface the DM in
            // the sidebar so the conversation is effectively hidden.
            if db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await? {
                continue;
            }
            if let Some(record) = db::auth::find_user_by_id(&state.auth, peer_id).await? {
                let effective = effective_status(state, &record.id, &record.status);
                sidebar_peers.push(SidebarPeer {
                    id: record.id.clone(),
                    username: record.username.clone(),
                    display_name: record.display_name.clone(),
                    avatar_ext: record.avatar_ext.clone(),
                    unread: *dm_unreads_by_room.get(&room.id).unwrap_or(&0),
                    status: effective,
                    custom_status: record.custom_status.clone(),
                    mute_mode: dm_mute_modes
                        .get(&room.id)
                        .copied()
                        .unwrap_or(db::notifications::MuteMode::None)
                        .as_str()
                        .to_string(),
                    active: false,
                });
            }
        }
        (Vec::new(), sidebar_peers)
    };

    Ok((sidebar_rooms, sidebar_peers))
}

/// Convenience wrapper that returns sidebar lists plus the switcher entries
/// in one call. Most page handlers want all three for layout.html.
pub(crate) async fn load_chrome(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<(Vec<SidebarRoom>, Vec<SidebarPeer>, Vec<SwitcherEntry>), AppError> {
    let (rooms, peers) = load_sidebar(state, user, current_enclave).await?;
    let switcher = load_switcher(state, user, current_enclave).await?;
    Ok((rooms, peers, switcher))
}

/// Resolve a room's enclave so the sidebar/switcher can highlight the right
/// icon when the caller is viewing /room/{id}. Returns None for DMs and
/// rooms without an enclave (which should not exist outside DMs).
pub(crate) async fn enclave_for_room(
    state: &AppState,
    room_id: i64,
) -> Result<Option<i64>, AppError> {
    use sqlx::Row;
    let row = sqlx::query("SELECT enclave_id FROM rooms WHERE id=?")
        .bind(room_id)
        .fetch_optional(&state.chat)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<i64>, _>("enclave_id")))
}

/// Build the leftmost switcher column: a Home entry plus one icon per
/// enclave the caller is a member of. The `current_enclave` argument
/// highlights the active icon (None = Home).
pub(crate) async fn load_switcher(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<Vec<SwitcherEntry>, AppError> {
    let is_admin = user.role == "admin";

    let dm_unread: i64 = db::chat::list_dm_unread_counts(&state.chat, &user.id)
        .await?
        .iter()
        .map(|(_, c)| *c)
        .sum();
    let pending_invites = db::enclave::list_invitations_for_user(&state.chat, &user.id)
        .await?
        .len() as i64;

    let mut entries = Vec::new();
    entries.push(SwitcherEntry {
        id: None,
        label: "Home".to_string(),
        initial: "H".to_string(),
        unread: dm_unread,
        pending_invites,
        active: current_enclave.is_none(),
    });

    let enclaves = db::enclave::list_enclaves_for_user(&state.chat, &user.id).await?;
    let _ = is_admin;
    for e in enclaves {
        let initial = e
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        entries.push(SwitcherEntry {
            id: Some(e.id),
            label: e.name,
            initial,
            // Per-enclave unread aggregation is added in Phase 4 alongside
            // EnclaveRoomAdded broadcasts; for now the icon shows no badge.
            unread: 0,
            pending_invites: 0,
            active: current_enclave == Some(e.id),
        });
    }

    Ok(entries)
}

/// Broadcast a `NewMessage` (or other room-wide) event to every user that has
/// the room visible in their sidebar - room members for private/DM rooms, all
/// connected users for public rooms. Each recipient's WebSocket handler then
/// decides how to render based on whether the connection is currently
/// subscribed to the room (open in the foreground) or not (sidebar-only,
/// renders an unread bump). Replaces the narrower `broadcast_to_room` so
/// sidebar badges update live for users who are not actively viewing the
/// room when a new message arrives.
pub(crate) async fn broadcast_room_message(
    state: &AppState,
    room: &Room,
    event: &ChatEvent,
) -> Result<(), AppError> {
    let recipients: Vec<String> = match room.room_type.as_str() {
        "public" => state.hub.list_connected_users(),
        _ => db::chat::list_room_member_ids(&state.chat, room.id).await?,
    };
    for uid in recipients {
        // OOB DOM updates always deliver - DND only suppresses attention-
        // grabbing notifications (push/toast/sound). When that delivery path
        // ships it should consult should_notify(state, uid) below before
        // firing.
        state.hub.broadcast_to_user(&uid, event);
    }
    Ok(())
}

/// True when `user_id` is currently accepting attention-grabbing
/// notifications. Returns false for DND users. v1 has no push/toast/sound
/// path; this helper is the seam future delivery code must consult.
#[allow(dead_code)]
pub(crate) async fn should_notify(state: &AppState, user_id: &str) -> bool {
    !db::auth::is_user_dnd(&state.auth, user_id)
        .await
        .unwrap_or(false)
}

/// Fallback handler for routes that did not match any registered path. Renders
/// a friendly 404 inside the normal app shell when the caller is logged in,
/// otherwise sends them to /login like any other authenticated route would.
pub(crate) async fn handle_not_found(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(user) = user else {
        return Redirect::to("/login").into_response();
    };
    let chrome = match load_chrome(&state, &user, None).await {
        Ok(c) => c,
        Err(_) => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    let (sidebar_rooms, sidebar_peers, switcher) = chrome;
    let page = NotFoundPage {
        user: &user,
        path: Some(uri.path().to_string()),
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    };
    match page.render() {
        Ok(body) => (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

pub fn build_router(state: AppState) -> Router {
    let router = Router::new()
        .route("/", get(home::get_home))
        .route("/login", get(auth::get_login).post(auth::post_login))
        .route(
            "/login/2fa",
            get(two_factor::get_login_challenge).post(two_factor::post_login_challenge),
        )
        .route(
            "/login/recovery",
            get(two_factor::get_login_recovery).post(two_factor::post_login_recovery),
        )
        .route("/logout", get(auth::get_logout))
        .route("/room/{room_id}", get(room::get_room))
        .route("/room/{room_id}/messages", post(room::post_message))
        .route(
            "/room/{room_id}/notify-prefs",
            post(notify_prefs::post_notify_prefs),
        )
        .route(
            "/room/{room_id}/thread/{message_id}",
            get(room::get_thread_panel),
        )
        .route(
            "/room/{room_id}/thread/{parent_id}/messages",
            post(room::post_thread_reply),
        )
        .route("/thread-panel", delete(room::close_thread_panel))
        .route("/dm/{peer_id}", get(dm::get_dm))
        .route("/dm/{peer_id}/mute", post(dm_mute::post_dm_mute))
        .route("/dm/{peer_id}/pins", get(pinned::get_dm_pins))
        .route("/room/{room_id}/pins", get(pinned::get_room_pins))
        .route(
            "/messages/{message_id}/pin",
            post(pinned::post_pin).delete(pinned::delete_pin),
        )
        .route(
            "/messages/{message_id}",
            get(room::get_single_message)
                .patch(room::patch_message)
                .delete(room::delete_message),
        )
        .route("/messages/{message_id}/edit", get(room::get_edit_form))
        .route(
            "/messages/{message_id}/reactions/picker",
            get(reactions::get_picker),
        )
        .route(
            "/messages/{message_id}/reactions/cancel",
            get(reactions::cancel_picker),
        )
        .route(
            "/messages/{message_id}/reactions/{emoji}",
            post(reactions::toggle_reaction),
        )
        .route("/search", get(search::get_search))
        .route("/users/search", get(users::get_user_search))
        .route("/users/mentions", get(mentions::get_autocomplete))
        .route("/users/{user_id}/block", post(users::post_block))
        .route("/users/{user_id}/unblock", post(users::post_unblock))
        .route(
            "/settings",
            get(settings::get_settings).post(settings::post_settings),
        )
        .route(
            "/settings/2fa/setup",
            get(two_factor::get_setup).post(two_factor::post_setup),
        )
        .route(
            "/settings/blocked",
            get(settings::get_blocked_list).post(settings::post_block_by_username),
        )
        .route("/settings/profile", post(settings::post_profile))
        .route(
            "/settings/avatar/delete",
            post(settings::post_avatar_delete),
        )
        .route("/avatars/{user_id}", get(avatar::get_avatar))
        .route("/sw.js", get(push::get_service_worker))
        .route("/push/vapid-public-key", get(push::get_vapid_public_key))
        .route("/push/subscribe", post(push::post_subscribe))
        // The upload handler streams + caps bytes itself based on settings,
        // so disable Axum's default 2 MiB body cap on this route only.
        .route(
            "/api/upload",
            post(uploads::post_upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/files/{id}", get(uploads::get_file))
        .route("/api/unfurl", get(unfurl::get_unfurl))
        .route("/status", post(status::post_status))
        .route("/status/picker", get(status::get_picker))
        .route("/status/cancel", get(status::cancel_picker))
        .route("/ws", get(ws::ws_handler))
        .route("/version", get(get_version))
        .merge(enclave::router());

    #[cfg(feature = "standalone")]
    let router = router
        .route(
            "/register",
            get(auth::get_register).post(auth::post_register),
        )
        .merge(admin::router());

    #[cfg(feature = "saas")]
    let router = router.merge(saas_auth::router());

    router
        .nest_service("/assets", ServeDir::new("server/assets"))
        .fallback(handle_not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            enforce_2fa_enrollment,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Public diagnostic endpoint. Returns the build metadata baked in by
/// `server/build.rs` so operators can confirm exactly what is running.
async fn get_version() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "version": version::VERSION,
        "git_version": version::GIT_VERSION,
        "git_hash": version::GIT_HASH,
        "build_date": version::BUILD_DATE,
    }))
}
