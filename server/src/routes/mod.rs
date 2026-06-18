use askama::Template;
use axum::extract::{OriginalUri, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post, put},
    Router,
};
use std::collections::HashMap;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::{enforce_2fa_enrollment, enforce_maintenance_mode, inject_user, OptionalUser};
use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;
use crate::version;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::message_actor::MessageActor;
use crate::views::not_found::NotFoundPage;
use crate::ws::events::ChatEvent;

mod account;
mod activity;
#[cfg(feature = "standalone")]
mod admin;
mod api;
mod api_tokens;
mod auth;
mod avatar;
mod bookmarks;
mod branding;
mod bridge_avatar_media;
#[cfg(feature = "standalone")]
mod bunyip_sso;
mod call;
mod channel_complete;
mod custom_emojis;
mod dm;
mod dm_mute;
mod drafts;
mod email_inboxes;
mod emoji_complete;
mod enclave;
mod export;
mod feeds;
mod forward;
mod home;
mod inbox;
mod mentions;
mod notify_prefs;
mod pinned;
mod polls;
mod push;
mod reactions;
mod read_all;
mod reminders;
mod retention;
pub(crate) mod room;
mod room_info;
mod room_rbac;
#[cfg(feature = "saas")]
mod saas_auth;
mod scheduled;
mod search;
mod settings;
mod sidebar_categories;
mod slash;
mod starred_rooms;
mod status;
mod switcher;
mod unfurl;
mod uploads;
mod user_groups;
mod users;
mod webhooks;
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

/// LC-266: build the per-reaction "who reacted" tooltip text, aligned to
/// `reactions`. Resolves every distinct reactor id across the slice in ONE
/// auth query, then comma-joins each reaction's reactor display names (capped).
/// Best-effort: a label-lookup error yields empty titles (no tooltip), never an
/// `Err`, so the reaction bar always renders.
pub(crate) async fn build_reactor_titles(
    state: &AppState,
    reactions: &[crate::models::reaction::Reaction],
) -> Vec<String> {
    use std::collections::HashSet;
    let mut id_set: HashSet<&str> = HashSet::new();
    for r in reactions {
        for id in &r.reactor_ids {
            id_set.insert(id.as_str());
        }
    }
    if id_set.is_empty() {
        return vec![String::new(); reactions.len()];
    }
    let ids: Vec<&str> = id_set.into_iter().collect();
    let labels = db::auth::display_names_for_ids(&state.auth, &ids)
        .await
        .unwrap_or_default();
    reactions
        .iter()
        .map(|r| reactor_title_from_labels(&r.reactor_ids, &labels))
        .collect()
}

/// Comma-join reactor display names (display_name else username) for one
/// reaction, capped so the rendered `title` attribute stays bounded.
fn reactor_title_from_labels(
    reactor_ids: &[String],
    labels: &HashMap<String, (String, Option<String>)>,
) -> String {
    const CAP: usize = 20;
    let names: Vec<String> = reactor_ids
        .iter()
        .map(|id| match labels.get(id) {
            Some((username, display_name)) => match display_name.as_deref() {
                Some(n) if !n.trim().is_empty() => n.to_string(),
                _ => username.clone(),
            },
            None => id.clone(),
        })
        .collect();
    if names.len() <= CAP {
        names.join(", ")
    } else {
        format!("{}, +{}", names[..CAP].join(", "), names.len() - CAP)
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
    /// LC-73: true for bot authors, drives the "bot" badge.
    pub is_bot: bool,
    /// LC-77: actor identity (User / Webhook / EmailInbox). For synthetic
    /// actors (Webhook, EmailInbox), the variant payload carries the
    /// configured avatar URL. The display name is populated into
    /// `username` above for all three variants so the template can read
    /// it uniformly.
    pub actor: MessageActor,
}

impl AuthorMeta {
    pub fn unknown() -> Self {
        Self {
            username: "(unknown)".to_string(),
            display_name: None,
            avatar_ext: None,
            status: db::auth::STATUS_ACTIVE.to_string(),
            custom_status: None,
            is_bot: false,
            actor: MessageActor::User,
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
            is_bot: r.is_bot,
            actor: MessageActor::User,
        }
    }
}

/// Resolve the author identity for a message that may be authored by a real
/// user, an incoming webhook (LC-74), an email-ingress inbox (LC-77), or a
/// protocol bridge (LC-78). Synthetic-actor messages (any of `webhook_id`,
/// `email_inbox_id`, `bridge_id` set) render with the actor's name + avatar
/// and no DM link.
///
/// At most one of `webhook_id`, `email_inbox_id`, `bridge_id` is `Some` for
/// any given message; if multiple are set, the older surfaces win in source
/// order (webhook > email > bridge), so a future migration that accidentally
/// double-tags a row does not silently flip rendering identity.
///
/// `bridge_foreign_name` / `bridge_kind` carry the bridge snapshot (LC-78);
/// they are read directly from `RawMessage`, never joined back to the bridges
/// row, because the foreign actor set is open-ended and the snapshot must
/// survive bridge-row removal under stop-new lifecycle.
///
/// The arg count exceeds clippy's default (9 vs 7) because each `Option<i64>`
/// / `Option<&str>` is read straight off a `RawMessage` row and grouping them
/// into a synthetic-actor struct would force every callsite to build that
/// struct from the same six fields. Suppress here rather than refactor.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_msg_author(
    state: &AppState,
    user_id: &str,
    webhook_id: Option<i64>,
    email_inbox_id: Option<i64>,
    bridge_id: Option<i64>,
    bridge_foreign_name: Option<&str>,
    bridge_kind: Option<&str>,
    bridge_foreign_avatar: Option<&str>,
    viewer_id: &str,
    // LC-321: the room the message belongs to, so the real-user branch can
    // apply a per-room nickname. Synthetic-actor branches ignore it.
    room_id: i64,
) -> Result<AuthorMeta, AppError> {
    if let Some(wid) = webhook_id {
        let (name, avatar_url) = db::webhooks::identity(&state.chat, wid)
            .await?
            .map(|w| (w.name, w.avatar_url))
            .unwrap_or_else(|| ("webhook".to_string(), None));
        return Ok(AuthorMeta {
            username: name,
            display_name: None,
            avatar_ext: None,
            status: db::auth::STATUS_ACTIVE.to_string(),
            custom_status: None,
            is_bot: false,
            actor: MessageActor::Webhook(avatar_url),
        });
    }
    if let Some(eid) = email_inbox_id {
        let (name, avatar_url) = db::email_inbox::identity(&state.chat, eid)
            .await?
            .map(|w| (w.name, w.avatar_url))
            .unwrap_or_else(|| ("email".to_string(), None));
        return Ok(AuthorMeta {
            username: name,
            display_name: None,
            avatar_ext: None,
            status: db::auth::STATUS_ACTIVE.to_string(),
            custom_status: None,
            is_bot: false,
            actor: MessageActor::EmailInbox(avatar_url),
        });
    }
    // Gate on the snapshot NAME, not bridge_id: under stop-new removal,
    // `ON DELETE SET NULL` clears bridge_id while the snapshot strings
    // persist, and the render still needs to show "alice (via matrix)" for
    // those historical messages. If foreign_name is set and bridge_id is
    // NULL, the message came from a removed bridge; it stays a bridge actor.
    if let Some(name) = bridge_foreign_name {
        let _ = bridge_id; // intentionally not gated on
        let kind = bridge_kind.unwrap_or("bridge").to_string();
        let name = name.to_string();
        // LC-78-AVATAR-PROXY: pass the cache key through to the template
        // so it renders <img src=/media/bridge-avatar-proxy/{hash}>. The
        // proxy endpoint 404s when fetch is still pending / failed; the
        // template's onerror falls back to initials. None when the message
        // had no foreign avatar OR the proxy gate was off at submit time.
        let avatar_url = bridge_foreign_avatar.map(str::to_string);
        return Ok(AuthorMeta {
            username: name,
            display_name: None,
            avatar_ext: None,
            status: db::auth::STATUS_ACTIVE.to_string(),
            custom_status: None,
            is_bot: false,
            actor: MessageActor::Bridge(crate::views::message_actor::BridgeActorMeta {
                kind,
                avatar_url,
            }),
        });
    }
    load_author_meta(state, user_id, viewer_id, room_id).await
}

/// LC-78: synthesize the `actor` block for the LC-75 outgoing-webhook
/// payload. Describes the ORIGINAL author of the message, not whoever
/// triggered the current event (a `message.edited` event still names the
/// message's author here, not the editor). That matches the bridge
/// daemon's loop-back concern: the daemon needs to ignore events for
/// messages it itself produced.
///
/// **Operator-visible contract.** A daemon MUST self-filter
/// `actor.kind == "<its own kind>" && actor.<its own id field> == <its id>`
/// on every outgoing-webhook event or it creates an infinite cross-network
/// amplification loop. The server provides the field; loop-break is the
/// daemon's responsibility (see docs/protocol-bridges.md).
///
/// The bridge arm is gated on `bridge_foreign_name.is_some()`, NOT on
/// `bridge_id.is_some()`, mirroring the render resolver. Under stop-new
/// removal `bridge_id` is nulled while the snapshot strings persist; the
/// resolver and the loop-break payload must stay consistent so removed-
/// bridge messages do not silently flip kind from `bridge` to `user`
/// in mid-life.
pub(crate) fn outgoing_actor(
    user_id: &str,
    webhook_id: Option<i64>,
    email_inbox_id: Option<i64>,
    bridge_id: Option<i64>,
    bridge_foreign_name: Option<&str>,
) -> serde_json::Value {
    if let Some(wid) = webhook_id {
        serde_json::json!({ "kind": "webhook", "webhook_id": wid })
    } else if let Some(eid) = email_inbox_id {
        serde_json::json!({ "kind": "email_inbox", "email_inbox_id": eid })
    } else if bridge_foreign_name.is_some() {
        serde_json::json!({ "kind": "bridge", "bridge_id": bridge_id })
    } else {
        serde_json::json!({ "kind": "user", "user_id": user_id })
    }
}

pub(crate) async fn load_author_meta(
    state: &AppState,
    user_id: &str,
    viewer_id: &str,
    room_id: i64,
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
    // LC-321: a per-room nickname overrides the global display_name (and the
    // username fallback) for this author's messages in this room. `username`
    // is left untouched so @mentions / DM links / avatar initials still work.
    // Synthetic actors (webhook/email/bridge) return before this in
    // `resolve_msg_author`, so they never pick up a nickname.
    if let Some(nick) = db::room_nicknames::get(&state.chat, room_id, user_id).await? {
        if !nick.trim().is_empty() {
            meta.display_name = Some(nick);
        }
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
    is_bookmarked: bool,
) -> Result<crate::views::room::MessageView, AppError> {
    use crate::views::room::{MessageView, ReactionView};
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let meta = resolve_msg_author(
        state,
        &m.user_id,
        m.webhook_id,
        m.email_inbox_id,
        m.bridge_id,
        m.bridge_foreign_name.as_deref(),
        m.bridge_kind.as_deref(),
        m.bridge_foreign_avatar.as_deref(),
        &viewer.id,
        m.room_id,
    )
    .await?;
    let can_edit = m.user_id == viewer.id;
    let can_delete = m.user_id == viewer.id
        || db::room_rbac::is_room_moderator(&state.chat, m.room_id, &viewer.id, &viewer.role)
            .await?;
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
    let reaction_counts = db::chat::list_reactions(&state.chat, m.id, &viewer.id).await?;
    let reactor_titles = build_reactor_titles(state, &reaction_counts).await;
    let reactions: Vec<ReactionView> = reaction_counts
        .into_iter()
        .zip(reactor_titles)
        .map(|(r, title)| {
            ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &custom_emojis)
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
    let quote_preview = match m.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid).await?,
        None => None,
    };
    let channels = channel_refs_for_room(state, m.room_id, viewer).await?;
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
        // LC-244: per-message render; the client inserts live day dividers.
        day_label: None,
        reply_count,
        parent_id: m.parent_id,
        attachments,
        mentions,
        is_pinned,
        is_bookmarked,
        custom_emojis,
        channels,
        quote_preview,
        is_system: m.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, m.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        author_is_bot: meta.is_bot,
        actor: meta.actor.clone(),
    })
}

/// Refresh the caller's `last_active_at` and, if that bumped the row from
/// idle back to active, broadcast `UserStatusChanged` so subscribers can
/// update their UI. DND status is sticky and never flips here.
///
/// Hot path (active or DND user): defer the column write to the bg
/// writer. The handler returns immediately and never touches the
/// SQLite pool, so a burst of WS messages cannot exhaust connections.
///
/// Cold path (no prior entry, or entry older than the debounce window):
/// run `touch_user_activity` inline so an idle->active flip is observed
/// and broadcast right away. The cold path runs at most once per user
/// per debounce window, which is rare even under load.
pub(crate) async fn touch_user_and_maybe_broadcast(state: &AppState, user_id: &str) {
    use std::time::{Duration, Instant};
    const ACTIVITY_DEBOUNCE: Duration = Duration::from_secs(30);

    let stale = match state.activity_ledger.get(user_id) {
        Some(prev) => Instant::now().duration_since(*prev) >= ACTIVITY_DEBOUNCE,
        None => true,
    };
    if !stale {
        // Routine "stay active" touch: send to the background writer
        // and return. The bg worker batches these into one UPDATE per
        // tick across all touched users.
        state.bg.touch_user(user_id);
        return;
    }
    state
        .activity_ledger
        .insert(user_id.to_string(), Instant::now());

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
) -> Result<
    (
        Vec<SidebarCategoryGroup>,
        Vec<SidebarRoom>,
        Vec<SidebarPeer>,
        Vec<SidebarRoom>,
        Vec<SidebarPeer>,
        bool,
        Option<i64>,
    ),
    AppError,
> {
    let is_admin = user.role == "admin";

    // LC-239: rooms/DMs in which the viewer has a fresh unsent draft, so each
    // sidebar row can render the pencil draft indicator. 60-day freshness
    // matches the per-room render purge in `db::drafts::get_fresh_or_purge`.
    let draft_room_ids = db::drafts::room_ids_with_drafts(&state.chat, &user.id, 60).await?;

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
                has_draft: draft_room_ids.contains(&r.id),
                id: r.id,
                name: r.name,
                is_voice: r.is_voice,
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
                    dm_room_id: room.id,
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
                    has_draft: draft_room_ids.contains(&room.id),
                });
            }
        }
        (Vec::new(), sidebar_peers)
    };

    // Categories are enclave-scoped (LC-79 redesign): only fetch +
    // bucket when the user is viewing an enclave. Home / DM views have
    // no notion of categories and render the existing flat rooms +
    // peers shape.
    let (sidebar_categories, uncategorized) = if let Some(eid) = current_enclave {
        let mut category_meta =
            db::sidebar_categories::list_categories_for_enclave(&state.chat, eid).await?;
        let assignments =
            db::sidebar_categories::room_assignments_for_enclave(&state.chat, eid).await?;
        let collapsed =
            db::sidebar_categories::list_collapsed_for_user(&state.auth, &user.id).await?;

        let mut by_category: HashMap<i64, Vec<(i64, SidebarRoom)>> = HashMap::new();
        let mut uncategorized: Vec<SidebarRoom> = Vec::with_capacity(sidebar_rooms.len());
        for room in sidebar_rooms {
            match assignments.get(&room.id) {
                Some((cat_id, pos)) => {
                    by_category.entry(*cat_id).or_default().push((*pos, room));
                }
                None => uncategorized.push(room),
            }
        }

        category_meta.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
        let sidebar_categories: Vec<SidebarCategoryGroup> = category_meta
            .into_iter()
            .map(|cat| {
                let mut rooms = by_category.remove(&cat.id).unwrap_or_default();
                rooms.sort_by(|a, b| a.0.cmp(&b.0));
                let rooms: Vec<SidebarRoom> = rooms.into_iter().map(|(_, r)| r).collect();
                let unread_total: i64 = rooms
                    .iter()
                    .filter(|r| r.mute_mode == "none")
                    .map(|r| r.unread)
                    .sum();
                let mention_total: i64 = rooms.iter().map(|r| r.mentions).sum();
                SidebarCategoryGroup {
                    id: cat.id,
                    enclave_id: cat.enclave_id,
                    name: cat.name,
                    collapsed: collapsed.contains(&cat.id),
                    rooms,
                    unread_total,
                    mention_total,
                }
            })
            .collect();
        (sidebar_categories, uncategorized)
    } else {
        (Vec::new(), sidebar_rooms)
    };

    // Whether the calling user can edit shared category state in the
    // currently-viewed enclave. Site admins always can; enclave admins
    // and owners can. Read once here so every view that wants to gate
    // an "Add category" / "Rename" / "Delete" affordance does not have
    // to redo the query.
    let can_manage_sidebar_categories = match current_enclave {
        Some(eid) => {
            let membership = db::enclave::get_membership(&state.chat, eid, &user.id).await?;
            crate::perms::enclave_can_manage(membership.map(|m| m.role), &user.role)
        }
        None => false,
    };

    // LC-80: split starred rooms / DMs out into their own top-of-sidebar
    // sections. Pull the per-user star set + positions once and bucket
    // both the room and DM lists. A starred categorized room is shown
    // ONLY in the Starred section (not duplicated in its category) so
    // the user sees each row exactly once.
    let starred_ids = db::starred_rooms::starred_room_ids(&state.auth, &user.id).await?;
    let star_positions = db::starred_rooms::star_positions(&state.auth, &user.id).await?;
    let (sidebar_starred_rooms, uncategorized) =
        split_starred_rooms(uncategorized, &starred_ids, &star_positions);
    let mut sidebar_categories = sidebar_categories;
    let mut starred_from_categories: Vec<SidebarRoom> = Vec::new();
    for cat in sidebar_categories.iter_mut() {
        let mut kept = Vec::with_capacity(cat.rooms.len());
        for room in cat.rooms.drain(..) {
            if starred_ids.contains(&room.id) {
                starred_from_categories.push(room);
            } else {
                kept.push(room);
            }
        }
        cat.rooms = kept;
        // Recompute aggregates after pulling out starred rooms so the
        // collapsed-header counts match the rooms still in the category.
        cat.unread_total = cat
            .rooms
            .iter()
            .filter(|r| r.mute_mode == "none")
            .map(|r| r.unread)
            .sum();
        cat.mention_total = cat.rooms.iter().map(|r| r.mentions).sum();
    }
    let mut sidebar_starred_rooms = sidebar_starred_rooms;
    sidebar_starred_rooms.extend(starred_from_categories);
    sidebar_starred_rooms.sort_by_key(|r| star_positions.get(&r.id).copied().unwrap_or(i64::MAX));

    let (sidebar_starred_peers, sidebar_peers) =
        split_starred_peers(sidebar_peers, &starred_ids, &star_positions);

    Ok((
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        uncategorized,
        sidebar_peers,
        can_manage_sidebar_categories,
        current_enclave,
    ))
}

/// Helper: split a SidebarRoom vec into (starred, rest) using the
/// user's per-room star set + position map.
fn split_starred_rooms(
    rooms: Vec<SidebarRoom>,
    starred_ids: &std::collections::HashSet<i64>,
    star_positions: &HashMap<i64, i64>,
) -> (Vec<SidebarRoom>, Vec<SidebarRoom>) {
    let (mut starred, rest): (Vec<_>, Vec<_>) =
        rooms.into_iter().partition(|r| starred_ids.contains(&r.id));
    starred.sort_by_key(|r| star_positions.get(&r.id).copied().unwrap_or(i64::MAX));
    (starred, rest)
}

/// Same split for DMs. DM rows are keyed on the DM peer's user_id in
/// SidebarPeer, but `starred_rooms.room_id` keys on the chat.db DM
/// room id. The DM SidebarPeer carries the peer id, not the room id;
/// the route layer must star by room id, so we surface the DM's
/// room id through SidebarPeer (added below) and filter on that.
fn split_starred_peers(
    peers: Vec<SidebarPeer>,
    starred_ids: &std::collections::HashSet<i64>,
    star_positions: &HashMap<i64, i64>,
) -> (Vec<SidebarPeer>, Vec<SidebarPeer>) {
    let (mut starred, rest): (Vec<_>, Vec<_>) = peers
        .into_iter()
        .partition(|p| starred_ids.contains(&p.dm_room_id));
    starred.sort_by_key(|p| {
        star_positions
            .get(&p.dm_room_id)
            .copied()
            .unwrap_or(i64::MAX)
    });
    (starred, rest)
}

/// Convenience wrapper that returns sidebar lists plus the switcher entries
/// in one call. Most page handlers want all three for layout.html.
pub(crate) async fn load_chrome(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<
    (
        Vec<SidebarCategoryGroup>,
        Vec<SidebarRoom>,
        Vec<SidebarPeer>,
        Vec<SidebarRoom>,
        Vec<SidebarPeer>,
        Vec<SwitcherEntry>,
        bool,
        Option<i64>,
    ),
    AppError,
> {
    let (
        categories,
        starred_rooms,
        starred_peers,
        rooms,
        peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = load_sidebar(state, user, current_enclave).await?;
    let switcher = load_switcher(state, user, current_enclave).await?;
    Ok((
        categories,
        starred_rooms,
        starred_peers,
        rooms,
        peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ))
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

/// LC-323: true when a room name can be `#`-linked. A channel token is
/// `#` + `[A-Za-z0-9_-]{1,64}`, so only names entirely within that charset
/// render as links (and are offered by the autocomplete). Names with spaces or
/// other characters are simply not `#`-linkable in v1.
pub(crate) fn is_linkable_channel_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// LC-323: rooms in `room_id`'s enclave that `viewer` can access and that have
/// a `#`-linkable name, as `ChannelRef`s. Empty for DMs / non-enclave rooms
/// (nothing to link against). Backs both the `#channel` autocomplete and the
/// render-time `#name` -> link resolution, so the two stay consistent.
pub(crate) async fn channel_refs_for_room(
    state: &AppState,
    room_id: i64,
    viewer: &User,
) -> Result<Vec<crate::views::channel_complete::ChannelRef>, AppError> {
    use crate::views::channel_complete::ChannelRef;
    let Some(enclave_id) = enclave_for_room(state, room_id).await? else {
        return Ok(Vec::new());
    };
    let is_admin = viewer.role == "admin";
    let rooms =
        db::chat::list_rooms_in_enclave(&state.chat, enclave_id, &viewer.id, is_admin).await?;
    Ok(rooms
        .into_iter()
        .filter(|r| is_linkable_channel_name(&r.name))
        .map(|r| ChannelRef {
            name: r.name,
            room_id: r.id,
        })
        .collect())
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

    // LC-141: render the branding logo on each switcher icon when one is
    // set. `resolve` returns the effective branding for the scope (an
    // enclave row, or the global row as fallback), so a logo URL is set
    // whenever the matching logo route would serve an image.
    // `?v=asset_version` matches the cache-bust the login page and admin
    // preview use, so the switcher logo refreshes on deploy alongside the
    // 1-day Cache-Control the logo route sets.
    let global_logo = db::branding::resolve(&state.chat, db::branding::Scope::Global)
        .await?
        .logo_upload_id
        .map(|_| format!("/branding/logo?v={}", state.asset_version));

    let mut entries = Vec::new();
    entries.push(SwitcherEntry {
        id: None,
        label: "Home".to_string(),
        initial: "H".to_string(),
        unread: dm_unread,
        pending_invites,
        active: current_enclave.is_none(),
        logo_url: global_logo,
        can_manage: false,
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
        let logo_url = db::branding::resolve(&state.chat, db::branding::Scope::Enclave(e.id))
            .await?
            .logo_upload_id
            .map(|_| format!("/enclave/{}/branding/logo?v={}", e.id, state.asset_version));
        // LC-143: settings gear visibility for the active enclave's tile.
        let role = db::enclave::get_membership(&state.chat, e.id, &user.id)
            .await?
            .map(|m| m.role);
        let can_manage = crate::perms::enclave_can_manage(role, &user.role);
        entries.push(SwitcherEntry {
            id: Some(e.id),
            label: e.name,
            initial,
            // Per-enclave unread aggregation is added in Phase 4 alongside
            // EnclaveRoomAdded broadcasts; for now the icon shows no badge.
            unread: 0,
            pending_invites: 0,
            active: current_enclave == Some(e.id),
            logo_url,
            can_manage,
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
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = chrome;
    let page = NotFoundPage {
        user: &user,
        path: Some(uri.path().to_string()),
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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
        // LC-22 cutover: /login renders the "Sign in with Bunyip" shell only.
        // No POST, no form. Username + password handlers are deleted.
        .route("/login", get(auth::get_login))
        .route("/logout", get(auth::get_logout))
        .route("/room/{room_id}", get(room::get_room))
        // LC-246: message permalink. Redirects to the canonical room/DM page
        // with a #msg-{id} fragment so the client scrolls to + highlights it.
        .route("/m/{message_id}", get(room::get_message_permalink))
        .route("/room/{room_id}/messages", post(room::post_message))
        .route("/room/{room_id}/preview", post(room::post_preview))
        .route("/messages/{message_id}/raw", get(room::get_message_raw))
        .route(
            "/messages/{message_id}/forward",
            get(forward::get_forward_picker),
        )
        .route(
            "/messages/{message_id}/forward/{dest_room_id}",
            post(forward::post_forward),
        )
        .route("/room/{room_id}/draft", put(drafts::put_draft))
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
        .route(
            "/room/{room_id}/thread/{parent_id}/follow",
            post(room::post_thread_follow).delete(room::delete_thread_follow),
        )
        .route("/thread-panel", delete(room::close_thread_panel))
        .route(
            "/room/{room_id}/composer-quote/{message_id}",
            get(room::get_composer_quote),
        )
        .route("/dm/{peer_id}", get(dm::get_dm))
        .route("/dm/{peer_id}/mute", post(dm_mute::post_dm_mute))
        .route("/dm/{peer_id}/pins", get(pinned::get_dm_pins))
        .route("/room/{room_id}/pins", get(pinned::get_room_pins))
        .route(
            "/messages/{message_id}/pin",
            post(pinned::post_pin).delete(pinned::delete_pin),
        )
        .route(
            "/messages/{message_id}/bookmark",
            post(bookmarks::post_bookmark).delete(bookmarks::delete_bookmark),
        )
        .route("/saved", get(bookmarks::get_saved))
        .route(
            "/scheduled",
            get(scheduled::get_scheduled).post(scheduled::post_scheduled),
        )
        .route(
            "/scheduled/{id}",
            axum::routing::patch(scheduled::patch_scheduled).delete(scheduled::delete_scheduled),
        )
        .route(
            "/reminders",
            get(reminders::get_reminders).post(reminders::post_reminder),
        )
        .route("/reminders/picker", get(reminders::get_picker))
        .route("/reminders/{id}", delete(reminders::delete_reminder))
        .route("/room/{room_id}/poll", post(polls::post_create))
        .route("/poll/{message_id}/vote", post(polls::post_vote))
        .route("/api/slash-commands", get(slash::get_autocomplete))
        .route("/inbox", get(inbox::get_inbox))
        .route("/activity", get(activity::get_activity))
        // LC-250: mark every conversation read in one action.
        .route("/read-all", post(read_all::post_read_all))
        .route("/room/{room_id}/read", post(read_all::post_room_read))
        .route(
            "/messages/{message_id}/unread",
            post(read_all::post_message_unread),
        )
        .route("/switcher", get(switcher::get_switcher))
        .route("/room/{room_id}/info", get(room_info::get_page))
        .route("/room/{room_id}/nickname", post(room_info::post_nickname))
        .route("/room/{room_id}/files", get(room_info::get_files))
        .route("/room/{room_id}/wiki/edit", get(room_info::get_wiki_edit))
        .route(
            "/room/{room_id}/wiki",
            get(room_info::get_wiki).patch(room_info::patch_wiki),
        )
        .route(
            "/room/{room_id}/description/edit",
            get(room_info::get_description_edit),
        )
        .route(
            "/room/{room_id}/description",
            get(room_info::get_description).patch(room_info::patch_description),
        )
        .route(
            "/room/{room_id}/moderators",
            get(room_rbac::get_page).post(room_rbac::post_grant),
        )
        .route(
            "/room/{room_id}/moderators/{user_id}",
            delete(room_rbac::delete_revoke),
        )
        .route(
            "/room/{room_id}/webhooks",
            get(webhooks::get_webhooks).post(webhooks::post_webhooks),
        )
        .route(
            "/room/{room_id}/webhooks/{wid}/revoke",
            post(webhooks::post_revoke),
        )
        .route(
            "/room/{room_id}/feeds",
            get(feeds::get_feeds).post(feeds::post_feeds),
        )
        .route(
            "/room/{room_id}/feeds/{fid}/revoke",
            post(feeds::post_revoke),
        )
        .route(
            "/room/{room_id}/email-inboxes",
            get(email_inboxes::get_email_inboxes).post(email_inboxes::post_email_inboxes),
        )
        .route(
            "/room/{room_id}/email-inboxes/{inbox_id}/revoke",
            post(email_inboxes::post_revoke),
        )
        .route(
            "/room/{room_id}/posting-policy",
            post(room_rbac::post_posting_policy),
        )
        .route(
            "/room/{room_id}/retention/preview",
            get(retention::get_preview),
        )
        .route("/room/{room_id}/retention", post(retention::post_set))
        .route(
            "/enclave/{enclave_id}/groups",
            post(user_groups::post_create),
        )
        .route(
            "/enclave/{enclave_id}/groups/{group_id}",
            axum::routing::patch(user_groups::patch_rename).delete(user_groups::delete_group),
        )
        .route(
            "/enclave/{enclave_id}/groups/{group_id}/members",
            post(user_groups::post_add_member),
        )
        .route(
            "/enclave/{enclave_id}/groups/{group_id}/members/search",
            get(user_groups::get_member_search),
        )
        .route(
            "/enclave/{enclave_id}/groups/{group_id}/members/{user_id}",
            delete(user_groups::delete_member),
        )
        .route(
            "/enclave/{enclave_id}/sidebar/categories",
            post(sidebar_categories::post_create),
        )
        .route(
            "/enclave/{enclave_id}/sidebar/categories/positions",
            axum::routing::patch(sidebar_categories::patch_category_positions),
        )
        .route(
            "/enclave/{enclave_id}/sidebar/categories/{category_id}",
            axum::routing::patch(sidebar_categories::patch_category)
                .delete(sidebar_categories::delete_category),
        )
        .route(
            "/enclave/{enclave_id}/sidebar/categories/{category_id}/positions",
            axum::routing::patch(sidebar_categories::patch_room_positions),
        )
        .route(
            "/enclave/{enclave_id}/sidebar/categories/{category_id}/rooms/{room_id}",
            axum::routing::patch(sidebar_categories::patch_room_assignment),
        )
        .route(
            "/enclave/{enclave_id}/sidebar/categories/rooms/{room_id}",
            delete(sidebar_categories::delete_room_assignment),
        )
        .route(
            "/sidebar/categories/{category_id}/collapse",
            axum::routing::patch(sidebar_categories::patch_collapse),
        )
        .route("/rooms/{room_id}/star", post(starred_rooms::post_toggle))
        .route(
            "/sidebar/stars/positions",
            axum::routing::patch(starred_rooms::patch_positions),
        )
        .route(
            "/messages/{message_id}",
            get(room::get_single_message)
                .patch(room::patch_message)
                .delete(room::delete_message),
        )
        .route("/messages/{message_id}/edit", get(room::get_edit_form))
        .route(
            "/messages/{message_id}/history",
            get(room::get_history_panel),
        )
        .route("/history-panel", delete(room::close_history_panel))
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
        .route("/searches", post(search::post_save))
        .route("/searches/delete", post(search::post_delete))
        .route("/users/search", get(users::get_user_search))
        .route("/users/{user_id}/hovercard", get(users::get_hovercard))
        .route("/users/mentions", get(mentions::get_autocomplete))
        .route(
            "/rooms/{room_id}/emoji-complete",
            get(emoji_complete::get_autocomplete),
        )
        .route(
            "/rooms/{room_id}/channel-complete",
            get(channel_complete::get_autocomplete),
        )
        .route(
            "/rooms/{room_id}/emoji-picker",
            get(emoji_complete::get_picker),
        )
        .route(
            "/api/rooms/{room_id}/broadcast-count",
            get(mentions::get_broadcast_count),
        )
        .route("/users/{user_id}/block", post(users::post_block))
        .route("/users/{user_id}/unblock", post(users::post_unblock))
        .route(
            "/settings",
            get(settings::get_settings).post(settings::post_settings),
        )
        .route("/settings/language", post(settings::post_language))
        .route("/settings/appearance", post(settings::post_appearance))
        .route("/settings/theme", post(settings::post_theme))
        .route("/settings/keywords", post(settings::post_keyword_add))
        .route(
            "/settings/keywords/delete",
            post(settings::post_keyword_delete),
        )
        .route("/settings/dnd", post(settings::post_dnd_schedule))
        .route("/settings/dnd/pause", post(settings::post_dnd_pause))
        .route("/settings/dnd/resume", post(settings::post_dnd_resume))
        .route("/settings/export-data", get(export::get_export))
        .route(
            "/settings/delete-account",
            post(account::post_delete_account),
        )
        .route(
            "/settings/blocked",
            get(settings::get_blocked_list).post(settings::post_block_by_username),
        )
        .route("/settings/profile", post(settings::post_profile))
        // LC-72: API-token management - available in both standalone and saas.
        .route(
            "/settings/api-tokens",
            get(api_tokens::get_api_tokens).post(api_tokens::post_api_tokens),
        )
        .route(
            "/settings/api-tokens/{id}/revoke",
            post(api_tokens::post_revoke),
        )
        .route(
            "/settings/avatar/delete",
            post(settings::post_avatar_delete),
        )
        .route(
            "/settings/sessions/{session_id}/revoke",
            post(settings::post_session_revoke),
        )
        .route("/avatars/{user_id}", get(avatar::get_avatar))
        .route("/sw.js", get(push::get_service_worker))
        .route("/push/vapid-public-key", get(push::get_vapid_public_key))
        .route("/push/subscribe", post(push::post_subscribe))
        .route("/push/apns", post(push::post_apns_register))
        .route("/push/fcm", post(push::post_fcm_register))
        // The upload handler streams + caps bytes itself based on settings,
        // so disable Axum's default 2 MiB body cap on this route only.
        .route(
            "/api/upload",
            post(uploads::post_upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/files/{id}", get(uploads::get_file))
        .route(
            "/media/bridge-avatar-proxy/{hash}",
            get(bridge_avatar_media::get_bridge_avatar),
        )
        .route("/api/emojis/{id}", get(custom_emojis::get_emoji))
        .route("/api/unfurl", get(unfurl::get_unfurl))
        .route("/status", post(status::post_status))
        .route("/status/picker", get(status::get_picker))
        .route("/status/cancel", get(status::cancel_picker))
        .route("/call/config", get(call::get_config))
        .route("/ws", get(ws::ws_handler))
        .route("/version", get(get_version))
        .route("/branding/logo", get(branding::get_global_logo))
        .route("/branding/favicon", get(branding::get_global_favicon))
        .route(
            "/enclave/{id}/branding/logo",
            get(branding::get_enclave_logo),
        )
        .merge(enclave::router());

    // LC-22 cutover: Bunyip SSO is the sole sign-in surface in the standalone
    // build. The saas build keeps its own JWT path via `saas_auth::router()`.
    #[cfg(feature = "standalone")]
    let router = router
        .route("/auth/bunyip/start", get(bunyip_sso::get_start))
        .route("/auth/bunyip/callback", get(bunyip_sso::get_callback))
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            enforce_maintenance_mode,
        ))
        // LC-96: branding CSS-var injection runs OUTSIDE the
        // enforcement gates so 503 / 2FA-redirect responses also
        // pick up the brand colors. Runs INSIDE `inject_user` so
        // the resolution still has request context.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            branding::inject_branding_css,
        ))
        // LC-100: resolve the UI locale into a task-local for template `| t`
        // filters. Inner of `inject_user` so the user's saved locale is known.
        .layer(middleware::from_fn(crate::auth::resolve_locale))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        // LC-72: the JSON API authenticates via bearer tokens (ApiAuth), not
        // the session cookie. Merge it AFTER the cookie / 2FA / maintenance /
        // branding layers so those never apply, but BEFORE TraceLayer so API
        // requests are still traced.
        .merge(api::router())
        .layer(TraceLayer::new_for_http())
        // LC-74: the incoming-webhook route carries the secret in its path.
        // Merge it AFTER TraceLayer so the secret is never written to request
        // logs (only the webhook id is logged, from the handler).
        .merge(webhooks::public_router())
        .merge(feeds::public_router())
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
