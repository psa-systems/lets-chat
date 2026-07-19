use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use std::collections::{HashMap, HashSet};

use crate::auth::OptionalUser;
use crate::db;
use crate::error::AppError;
use crate::last_visited;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::{
    CatchUpRow, DmRow, DraftRow, LandingPage, MentionRow, ThreadRow, WelcomePage,
};
use crate::views::html;

/// LC-575: how many rows each dashboard card shows at a glance.
const CARD_CAP: usize = 6;
/// LC-575: how deep into the unread inbox to scan for channel/DM previews.
const PREVIEW_SCAN: i64 = 80;

/// LC-575: collapse a message body to a single short preview line.
fn preview_of(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(120).collect();
    if flat.chars().count() > 120 {
        out.push('\u{2026}');
    }
    out
}

#[derive(Deserialize)]
pub struct HomeQuery {
    /// When `home=1`, render the Home pseudo-enclave directly and skip the
    /// `last_visited` redirect. The switcher's Home button uses this so that
    /// the user can explicitly go back to the DM hub from any room.
    #[serde(default)]
    pub home: Option<String>,
}

pub async fn get_home(
    State(state): State<AppState>,
    OptionalUser(maybe_user): OptionalUser,
    headers: HeaderMap,
    Query(q): Query<HomeQuery>,
) -> Result<Response, AppError> {
    // LC-470: logged-out visitors get the public marketing landing page
    // instead of being bounced to /login. Authenticated users fall through
    // to the existing chat-home behavior below.
    let user = match maybe_user {
        Some(u) => u,
        None => return render_landing(&state),
    };
    // LC-575: honor the "Open on" preference. `home` skips the last-visited
    // redirect so login lands on the dashboard; "last-room" (the NULL default)
    // keeps today's behavior for everyone who never opts in. `?home=1` (the
    // rail Home tile) forces the dashboard regardless of the pref.
    let land_on_home = user.home_landing_or_default() == "home";
    let force_home = q.home.as_deref() == Some("1");
    if !force_home && !land_on_home {
        if let Some(path) = last_visited::read(&headers) {
            if last_visited::is_safe_path(&path) && target_accessible(&state, &user, &path).await? {
                return Ok(Redirect::to(&path).into_response());
            }
        }
    }
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
    // LC-372: pending-invitation count for the welcome quick-action badge.
    let pending_invites = crate::db::enclave::list_invitations_for_user(&state.chat, &user.id)
        .await?
        .len();
    // LC-516: the user belongs to at least one enclave iff the switcher carries
    // any non-Home entry (the Home tile has id None; each enclave tile has id
    // Some(_)). load_chrome already fetched the enclave list to build the
    // switcher, so derive the flag from it instead of querying again.
    let has_enclaves = switcher.iter().any(|e| e.id.is_some());

    // LC-575: build the Home dashboard. These aggregate queries are workspace-
    // wide (unlike the enclave-scoped sidebar chrome above), so the dashboard
    // answers "what did I miss?" across every enclave. All read-only, capped,
    // and only run on this fall-through path (not on the last-room redirect).
    let (catch_up, mentions, threads, dms, drafts, show_dashboard) =
        build_dashboard(&state, &user).await?;

    let page = WelcomePage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        flash_error: None,
        pending_invites,
        has_enclaves,
        show_dashboard,
        catch_up: &catch_up,
        mentions: &mentions,
        threads: &threads,
        dms: &dms,
        drafts: &drafts,
    };
    let body = html(&page)?;
    Ok(body.into_response())
}

/// LC-575: assemble the Home dashboard cards for `user`. Returns the five card
/// row lists plus `show_dashboard` (false when the user has no accessible room
/// or DM, so the onboarding welcome renders instead). Faithful to the existing
/// helpers: workspace-wide unread counts drive the pills, the shared unread
/// inbox supplies channel/DM names + previews, and mentions / drafts / threads
/// each reuse their own helper.
#[allow(clippy::type_complexity)]
async fn build_dashboard(
    state: &AppState,
    user: &User,
) -> Result<
    (
        Vec<CatchUpRow>,
        Vec<MentionRow>,
        Vec<ThreadRow>,
        Vec<DmRow>,
        Vec<DraftRow>,
        bool,
    ),
    AppError,
> {
    let is_admin = user.role == "admin";
    let chat = &state.chat;

    // Workspace-wide unread counts (every accessible room / DM, even at 0).
    let room_counts: HashMap<i64, i64> =
        db::chat::list_room_unread_counts(chat, &user.id, is_admin)
            .await?
            .into_iter()
            .collect();
    let dm_counts: HashMap<i64, i64> = db::chat::list_dm_unread_counts(chat, &user.id)
        .await?
        .into_iter()
        .collect();
    let show_dashboard = !room_counts.is_empty() || !dm_counts.is_empty();

    // Catch up + Direct messages: newest-first unread across the workspace,
    // one row per room (first occurrence = newest preview). DM rows resolve the
    // peer for the /dm/{peer} deep-link and the avatar, mirroring the inbox.
    let inbox_rows = db::inbox::list_unread(chat, &user.id, is_admin, PREVIEW_SCAN, None).await?;
    let mut room_names: HashMap<i64, String> = HashMap::new();
    let mut catch_up: Vec<CatchUpRow> = Vec::new();
    let mut dms: Vec<DmRow> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for row in &inbox_rows {
        room_names
            .entry(row.room_id)
            .or_insert_with(|| row.room_name.clone());
        if !seen.insert(row.room_id) {
            continue;
        }
        if row.room_type == "dm" {
            if dms.len() >= CARD_CAP {
                continue;
            }
            let Some(peer_id) = db::chat::get_dm_peer(chat, row.room_id, &user.id).await? else {
                continue;
            };
            let Some(peer) = db::auth::find_user_by_id(&state.auth, &peer_id).await? else {
                continue;
            };
            let label = peer
                .display_name
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("@{}", peer.username));
            dms.push(DmRow {
                peer_id,
                label,
                username: peer.username,
                avatar_ext: peer.avatar_ext,
                unread: *dm_counts.get(&row.room_id).unwrap_or(&0),
            });
        } else if catch_up.len() < CARD_CAP {
            catch_up.push(CatchUpRow {
                room_id: row.room_id,
                name: row.room_name.clone(),
                preview: preview_of(&row.body),
                unread: *room_counts.get(&row.room_id).unwrap_or(&0),
            });
        }
    }

    // Mentions: per-room unread-mention counts. Resolve the room name from the
    // inbox map when we already have it, else one cheap lookup.
    let mut mentions: Vec<MentionRow> = Vec::new();
    for (room_id, count) in db::mentions::count_unread_mentions_per_room(chat, &user.id, is_admin)
        .await?
        .into_iter()
        .take(CARD_CAP)
    {
        let name = match room_names.get(&room_id) {
            Some(n) => n.clone(),
            None => match db::chat::get_room(chat, room_id).await? {
                Some(r) => r.name,
                None => continue,
            },
        };
        mentions.push(MentionRow {
            room_id,
            name,
            count,
        });
    }

    // Threads: followed threads carrying replies newer than the read watermark.
    let threads: Vec<ThreadRow> =
        db::thread_followers::followed_threads_with_unread(chat, &user.id, is_admin)
            .await?
            .into_iter()
            .take(CARD_CAP)
            .map(|d| ThreadRow {
                room_id: d.room_id,
                parent_id: d.parent_id,
                room_name: d.room_name,
                preview: preview_of(&d.parent_preview),
                unread: d.unread_replies,
            })
            .collect();

    // Drafts: rooms/DMs with a fresh unsent draft. Sort by id desc for a stable
    // order, then resolve each to the right deep-link + label.
    let mut draft_ids: Vec<i64> = db::drafts::room_ids_with_drafts(chat, &user.id, 60)
        .await?
        .into_iter()
        .collect();
    draft_ids.sort_unstable_by(|a, b| b.cmp(a));
    let mut drafts: Vec<DraftRow> = Vec::new();
    for room_id in draft_ids.into_iter().take(CARD_CAP) {
        let Some(room) = db::chat::get_room(chat, room_id).await? else {
            continue;
        };
        let (href, label) = if room.room_type == "dm" {
            let Some(peer_id) = db::chat::get_dm_peer(chat, room_id, &user.id).await? else {
                continue;
            };
            let label = match db::auth::find_user_by_id(&state.auth, &peer_id).await? {
                Some(p) => p
                    .display_name
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| format!("@{}", p.username)),
                None => continue,
            };
            (format!("/dm/{peer_id}"), label)
        } else {
            (format!("/room/{room_id}"), room.name)
        };
        drafts.push(DraftRow { href, label });
    }

    Ok((catch_up, mentions, threads, dms, drafts, show_dashboard))
}

/// LC-470: render the public marketing landing page. The Bunyip issuer host
/// is shown in the "you'll be redirected to ..." note, mirroring the login
/// page; it falls back to "Bunyip" when SSO is not configured.
fn render_landing(state: &AppState) -> Result<Response, AppError> {
    let host = state
        .bunyip_sso
        .as_ref()
        .map(|c| c.config.issuer.host_str().unwrap_or("Bunyip").to_string())
        .unwrap_or_else(|| "Bunyip".to_string());
    let page = LandingPage {
        asset_version: &state.asset_version,
        app_version: crate::version::VERSION,
        git_hash: crate::version::GIT_HASH,
        build_date: crate::version::BUILD_DATE,
        bunyip_issuer_host: &host,
    };
    Ok(html(&page)?.into_response())
}

async fn target_accessible(state: &AppState, user: &User, path: &str) -> Result<bool, AppError> {
    if let Some(rest) = path.strip_prefix("/room/") {
        let id: i64 = match rest.parse() {
            Ok(n) => n,
            Err(_) => return Ok(false),
        };
        return Ok(crate::db::chat::is_room_accessible(
            &state.chat,
            id,
            &user.id,
            user.role == "admin",
        )
        .await?);
    }
    if let Some(peer_id) = path.strip_prefix("/dm/") {
        if peer_id == user.id {
            return Ok(false);
        }
        // Require an existing DM room so we never lazily create one on the
        // home redirect. find_dm_room only returns rooms the caller is a
        // member of, so this is also an implicit access check.
        let dm = crate::db::chat::find_dm_room(&state.chat, &user.id, peer_id).await?;
        return Ok(dm.is_some());
    }
    Ok(false)
}
