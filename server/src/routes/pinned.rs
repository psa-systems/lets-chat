use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::pinned::{PinnedListPage, PinnedListRow, PinnedStripFragment, PinnedStripRow};
use crate::views::room::SingleMessageFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;
use askama::Template;

/// Header-strip render cap. Keeps the strip narrow even on rooms with many pins.
const STRIP_TOP_N: i64 = 3;
/// Per-pin snippet length in chars. Anything longer truncates at a word
/// boundary with an ellipsis.
const SNIPPET_MAX_CHARS: usize = 80;

/// Truncate a body to `SNIPPET_MAX_CHARS`, breaking at the last whitespace
/// when possible so the cut does not split a word. Newlines collapse to
/// single spaces - the strip line is single-line by design.
fn snippet_for(body: &str) -> String {
    let collapsed: String = body
        .chars()
        .map(|c| {
            if c.is_control() || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
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

/// `display_name` if non-empty, else `username`. Mirrors the rule used
/// elsewhere in the codebase (see `views::room::MessageView::label`).
fn label_for(username: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => username.to_string(),
    }
}

/// Build the pinned-strip fragment for a room. Resolves author + pinner
/// display labels in a single bulk auth lookup so the strip builder is
/// O(1) auth queries regardless of pin count. `oob` controls whether the
/// rendered HTML carries `hx-swap-oob="outerHTML"`.
pub(crate) async fn build_strip_fragment(
    state: &AppState,
    room_id: i64,
    pin_path: String,
    oob: bool,
) -> Result<PinnedStripFragmentOwned, AppError> {
    let pins = db::pinned::pins_for_room(&state.chat, room_id, STRIP_TOP_N).await?;
    let total_count = db::pinned::count_for_room(&state.chat, room_id).await?;

    let names = resolve_author_labels(state, &pins).await?;
    let top_pins: Vec<PinnedStripRow> = pins
        .iter()
        .map(|p| PinnedStripRow {
            message_id: p.message_id,
            author_label: names
                .get(&p.author_user_id)
                .cloned()
                .unwrap_or_else(|| p.author_user_id.clone()),
            snippet: snippet_for(&p.body),
            pinned_at: p.pinned_at.clone(),
        })
        .collect();
    let has_more = total_count > top_pins.len() as i64;
    Ok(PinnedStripFragmentOwned {
        room_id,
        total_count,
        has_more,
        pin_path,
        top_pins,
        oob,
    })
}

/// Owned analogue of `PinnedStripFragment` so the helper can return it
/// across await points without lifetime gymnastics. The Display impl
/// renders the underlying Askama fragment.
pub(crate) struct PinnedStripFragmentOwned {
    pub room_id: i64,
    pub total_count: i64,
    pub has_more: bool,
    pub pin_path: String,
    pub top_pins: Vec<PinnedStripRow>,
    pub oob: bool,
}

impl PinnedStripFragmentOwned {
    pub fn render(&self) -> Result<String, askama::Error> {
        use askama::Template;
        PinnedStripFragment {
            room_id: self.room_id,
            total_count: self.total_count,
            has_more: self.has_more,
            pin_path: &self.pin_path,
            top_pins: self
                .top_pins
                .iter()
                .map(|p| PinnedStripRow {
                    message_id: p.message_id,
                    author_label: p.author_label.clone(),
                    snippet: p.snippet.clone(),
                    pinned_at: p.pinned_at.clone(),
                })
                .collect(),
            oob: self.oob,
        }
        .render()
    }
}

/// One bulk auth query for every distinct user id appearing as either
/// an author or a pinner across `pins`. Returns a map keyed by user id
/// with the display label (display_name or username).
async fn resolve_author_labels(
    state: &AppState,
    pins: &[db::pinned::PinnedRow],
) -> Result<HashMap<String, String>, AppError> {
    let mut unique: HashSet<&str> = HashSet::new();
    for p in pins {
        unique.insert(p.author_user_id.as_str());
        unique.insert(p.pinned_by_user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let resolved = db::auth::display_names_for_ids(&state.auth, &ids).await?;
    Ok(resolved
        .into_iter()
        .map(|(id, (uname, dname))| (id, label_for(&uname, dname.as_deref())))
        .collect())
}

/// Resolve the message that the URL refers to, returning the room id or
/// 404 for nonexistent / soft-deleted messages. Common to the POST and
/// DELETE handlers.
async fn room_id_for_url_message(state: &AppState, message_id: i64) -> Result<i64, AppError> {
    db::pinned::room_for_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Verify the caller is allowed to act on the given room. Mirrors the
/// existing `is_room_accessible` check used by the page handlers and
/// the message handlers; the room-membership semantics already cover
/// both private rooms (member-only) and DMs (both parties).
async fn require_room_access(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Path used by the strip's "See all (N) pinned" link. Rooms get
/// `/room/{id}/pins`; DMs (room_type == 'dm') derive the peer id from
/// the room and use `/dm/{peer_id}/pins`.
async fn pin_path_for_room(
    state: &AppState,
    user: &User,
    room_id: i64,
) -> Result<String, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room_id, &user.id)
            .await?
            .ok_or(AppError::NotFound)?;
        Ok(format!("/dm/{peer_id}/pins"))
    } else {
        Ok(format!("/room/{room_id}/pins"))
    }
}

/// POST /messages/{message_id}/pin
///
/// Pin the message. Idempotent: pinning an already-pinned message is a
/// successful no-op. Returns the re-rendered message bubble (so the
/// acting tab's hover menu flips Pin -> Unpin via `hx-target=#msg-{id}`
/// outerHTML swap) plus the OOB pinned-strip fragment. Other tabs and
/// other room members receive a matching OOB pair via the WS broadcast.
pub async fn post_pin(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let room_id = room_id_for_url_message(&state, message_id).await?;
    require_room_access(&state, &user, room_id).await?;

    match db::pinned::pin_message(&state.chat, message_id, room_id, &user.id).await {
        Ok(()) => {}
        Err(sqlx::Error::Protocol(s)) if s.contains("pin cap reached") => {
            return Err(AppError::Conflict(format!(
                "Pin cap reached ({}). Unpin a message first.",
                db::pinned::MAX_PINS_PER_ROOM
            )));
        }
        Err(e) => return Err(e.into()),
    }

    state.hub.broadcast_to_room(
        room_id,
        &ChatEvent::MessagePinned {
            room_id,
            message_id,
            pinned_by: user.id.clone(),
        },
    );

    render_pin_response(&state, &user, room_id, message_id, true).await
}

/// DELETE /messages/{message_id}/pin
///
/// Unpin the message. Idempotent. Returns the re-rendered bubble plus the
/// OOB pinned-strip fragment (mirror of `post_pin`).
pub async fn delete_pin(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let room_id = room_id_for_url_message(&state, message_id).await?;
    require_room_access(&state, &user, room_id).await?;

    db::pinned::unpin_message(&state.chat, message_id).await?;

    state.hub.broadcast_to_room(
        room_id,
        &ChatEvent::MessageUnpinned {
            room_id,
            message_id,
        },
    );

    render_pin_response(&state, &user, room_id, message_id, false).await
}

/// Build the HTTP response body for pin/unpin: the re-rendered bubble
/// (primary swap target via the button's `hx-target`) concatenated with
/// the OOB strip fragment. The bubble carries `oob=false` so HTMX uses
/// the button's `hx-target="#msg-{id}"` to do the outerHTML swap.
async fn render_pin_response(
    state: &AppState,
    user: &User,
    room_id: i64,
    message_id: i64,
    is_pinned: bool,
) -> Result<Response, AppError> {
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &user.id, message_id).await?;
    let view =
        super::load_message_view_for_viewer(state, user, message_id, is_pinned, is_bookmarked)
            .await?;
    let bubble = SingleMessageFragment {
        message: &view,
        oob: false,
    }
    .render()?;
    let pin_path = pin_path_for_room(state, user, room_id).await?;
    let strip_html = build_strip_fragment(state, room_id, pin_path, true)
        .await?
        .render()?;
    Ok(Html(format!("{bubble}{strip_html}")).into_response())
}

/// GET /room/{room_id}/pins
///
/// Standalone page listing every pin in the room newest-first.
pub async fn get_room_pins(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_room_access(&state, &user, room_id).await?;
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let room_label = if room.room_type == "dm" {
        return Err(AppError::NotFound); // DMs use the dm-keyed URL.
    } else {
        format!("#{}", room.name)
    };
    render_pins_page(
        &state,
        &user,
        room_id,
        room_label,
        format!("/room/{room_id}"),
        super::enclave_for_room(&state, room_id).await?,
    )
    .await
}

/// GET /dm/{peer_id}/pins
///
/// Same as `get_room_pins` but resolves the DM room from the peer id.
pub async fn get_dm_pins(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(peer_id): Path<String>,
) -> Result<Html, AppError> {
    if peer_id == user.id {
        return Err(AppError::BadRequest(
            "cannot open pins for a DM with yourself".into(),
        ));
    }
    let peer_record = db::auth::find_user_by_id(&state.auth, &peer_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let peer: User = peer_record.into();
    let room = db::chat::find_dm_room(&state.chat, &user.id, &peer.id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_room_access(&state, &user, room.id).await?;
    let room_label = format!("@{}", peer.username);
    render_pins_page(
        &state,
        &user,
        room.id,
        room_label,
        format!("/dm/{peer_id}"),
        None,
    )
    .await
}

async fn render_pins_page(
    state: &AppState,
    user: &User,
    room_id: i64,
    room_label: String,
    back_path: String,
    current_enclave: Option<i64>,
) -> Result<Html, AppError> {
    let pins =
        db::pinned::pins_for_room(&state.chat, room_id, db::pinned::MAX_PINS_PER_ROOM).await?;
    let names = resolve_author_labels(state, &pins).await?;
    let rows: Vec<PinnedListRow> = pins
        .iter()
        .map(|p| PinnedListRow {
            message_id: p.message_id,
            author_label: names
                .get(&p.author_user_id)
                .cloned()
                .unwrap_or_else(|| p.author_user_id.clone()),
            pinner_label: names
                .get(&p.pinned_by_user_id)
                .cloned()
                .unwrap_or_else(|| p.pinned_by_user_id.clone()),
            pinned_at: p.pinned_at.clone(),
            body: p.body.clone(),
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
    ) = super::load_chrome(state, user, current_enclave).await?;
    let page = PinnedListPage {
        user,
        asset_version: &state.asset_version,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        room_label,
        back_path,
        pins: rows,
    };
    html(&page)
}
