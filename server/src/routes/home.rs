use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use std::collections::{HashMap, HashSet};

use crate::auth::OptionalUser;
use crate::db;
use crate::error::AppError;
use crate::i18n::translate_current;
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

/// LC-575 / LC-703: collapse a message body to a single short preview line via
/// the shared markdown one-liner (link -> label text, emphasis/heading/code
/// markers dropped), so raw markdown never leaks into the dashboard. Returns an
/// empty string for a message with no text body (an attachment-only message);
/// the caller then substitutes a per-type attachment label (LC-704).
fn preview_text(body: &str) -> String {
    crate::views::markdown::plain_line(body, 120)
}

/// LC-704: i18n key for the label shown in place of a bodyless message's
/// preview. Mirrors the timeline's own render precedence (audio -> video ->
/// image -> file; see `templates/partials/attachment.html`), classifying by the
/// first attachment so the label matches what the message leads with. An empty
/// slice (a bodyless message with no attachments we could load) falls back to
/// the generic label.
fn attachment_preview_key(atts: &[crate::models::Attachment]) -> &'static str {
    match atts.first() {
        Some(a) if a.is_audio() => "home-dash-preview-voice",
        Some(a) if a.is_video() => "home-dash-preview-video",
        Some(a) if a.is_image() => "home-dash-preview-image",
        Some(_) => "home-dash-preview-file",
        None => "home-dash-preview-attachment",
    }
}

/// LC-704: resolve a per-type attachment label key for each of `message_ids` by
/// bulk-loading their attachments once. Callers pass only the ids of bodyless
/// rows, so a dashboard whose previews are all text costs no extra query.
async fn attachment_preview_keys(
    state: &AppState,
    message_ids: &[i64],
) -> Result<HashMap<i64, &'static str>, AppError> {
    if message_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let atts = db::uploads::attachments_for_messages(&state.chat, message_ids).await?;
    Ok(message_ids
        .iter()
        .map(|id| {
            let key = attachment_preview_key(atts.get(id).map(Vec::as_slice).unwrap_or(&[]));
            (*id, key)
        })
        .collect())
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
    // LC-575: build the Home dashboard. These aggregate queries are workspace-
    // wide (unlike the enclave-scoped sidebar chrome above), so the dashboard
    // answers "what did I miss?" across every enclave. All read-only, capped,
    // and only run on this fall-through path (not on the last-room redirect).
    let (catch_up, mentions, threads, dms, drafts, show_dashboard) =
        build_dashboard(&state, &user).await?;
    // LC-705: offer the workspace "Catch me up" AI summary only when the surface
    // is available to this viewer AND there is unread to summarize. The card's
    // POST re-checks the gate, so this is a UI affordance, not the access check.
    let has_unread =
        !catch_up.is_empty() || !mentions.is_empty() || !threads.is_empty() || !dms.is_empty();
    let ai_catch_up = has_unread && super::ai_gate::llm_catch_up_available(&state, &user).await;
    // LC-707: personal "your week" glance for the greeting - the same two cheap
    // scalar aggregates the weekly recap DM uses, scoped to this user only.
    let week_messages = db::stats::weekly_message_count(&state.chat, &user.id).await?;
    let week_kudos = db::stats::weekly_kudos_received(&state.chat, &user.id).await?;

    // LC-726: admin-only pending-support glance for the dashboard card. The admin
    // support queue is standalone-only, so this only runs there and only for an
    // admin; everyone else gets an empty, zero-count card that renders nothing.
    // `mut` is only exercised in the standalone block below; saas leaves them at
    // the zero/empty defaults, so silence the lint there.
    #[allow(unused_mut)]
    let mut support_open_count = 0i64;
    #[allow(unused_mut)]
    let mut support_recent: Vec<crate::views::support::SupportView> = Vec::new();
    #[cfg(feature = "standalone")]
    if user.role == "admin" {
        support_open_count = db::support_tickets::count_open(&state.chat).await?;
        if support_open_count > 0 {
            support_recent = super::support::build_support_views(&state).await?;
            support_recent.truncate(3);
        }
    }

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
        show_dashboard,
        catch_up: &catch_up,
        mentions: &mentions,
        threads: &threads,
        dms: &dms,
        drafts: &drafts,
        ai_catch_up,
        week_messages,
        week_kudos,
        support_open_count,
        support_recent: &support_recent,
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
    // LC-704: source message id per catch-up row, kept parallel so a bodyless
    // preview can be back-filled with a per-type attachment label below.
    let mut catch_up_msg_ids: Vec<i64> = Vec::new();
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
            // LC-707: resolve the newest unread message's author for the row
            // avatar. One lookup per shown row (cap 6), mirroring how the DM rows
            // above resolve their peer. A deleted author falls back to initials.
            let author = db::auth::find_user_by_id(&state.auth, &row.author_user_id).await?;
            let (author_name, author_avatar_ext) = match author {
                Some(a) => (a.username, a.avatar_ext),
                None => (row.author_user_id.clone(), None),
            };
            catch_up_msg_ids.push(row.message_id);
            catch_up.push(CatchUpRow {
                room_id: row.room_id,
                name: row.room_name.clone(),
                preview: preview_text(&row.body),
                unread: *room_counts.get(&row.room_id).unwrap_or(&0),
                created_at: row.created_at.clone(),
                author_id: row.author_user_id.clone(),
                author_name,
                author_avatar_ext,
            });
        }
    }

    // LC-704: bodyless catch-up rows (attachment-only messages) show a per-type
    // label ("🖼 Image", "🎞 Video", "🎤 Voice message", "📎 File") in place of
    // the empty preview. One bulk attachment lookup for just the empty rows.
    let empty_catch: Vec<i64> = catch_up
        .iter()
        .zip(&catch_up_msg_ids)
        .filter(|(r, _)| r.preview.is_empty())
        .map(|(_, id)| *id)
        .collect();
    let catch_keys = attachment_preview_keys(state, &empty_catch).await?;
    for (r, id) in catch_up.iter_mut().zip(&catch_up_msg_ids) {
        if r.preview.is_empty() {
            let key = catch_keys
                .get(id)
                .copied()
                .unwrap_or("home-dash-preview-attachment");
            r.preview = translate_current(key);
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
    let mut threads: Vec<ThreadRow> =
        db::thread_followers::followed_threads_with_unread(chat, &user.id, is_admin)
            .await?
            .into_iter()
            .take(CARD_CAP)
            .map(|d| ThreadRow {
                room_id: d.room_id,
                parent_id: d.parent_id,
                room_name: d.room_name,
                preview: preview_text(&d.parent_preview),
                unread: d.unread_replies,
            })
            .collect();
    // LC-704: a bodyless thread root (attachment-only) shows a per-type label,
    // keyed off the root message id the row already carries (`parent_id`).
    let empty_threads: Vec<i64> = threads
        .iter()
        .filter(|r| r.preview.is_empty())
        .map(|r| r.parent_id)
        .collect();
    let thread_keys = attachment_preview_keys(state, &empty_threads).await?;
    for r in threads.iter_mut() {
        if r.preview.is_empty() {
            let key = thread_keys
                .get(&r.parent_id)
                .copied()
                .unwrap_or("home-dash-preview-attachment");
            r.preview = translate_current(key);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::attachment_preview_key;
    use crate::models::Attachment;

    fn att(mime: &str) -> Attachment {
        Attachment {
            id: 1,
            filename: "f".into(),
            mime_type: mime.into(),
            size_bytes: 1,
            url: "/api/files/1".into(),
            waveform: None,
            voice_duration: None,
            transcript: None,
            transcript_status: None,
            alt_text: None,
        }
    }

    #[test]
    fn preview_key_classifies_each_attachment_type() {
        assert_eq!(
            attachment_preview_key(&[att("image/png")]),
            "home-dash-preview-image"
        );
        assert_eq!(
            attachment_preview_key(&[att("image/gif")]),
            "home-dash-preview-image"
        );
        assert_eq!(
            attachment_preview_key(&[att("video/mp4")]),
            "home-dash-preview-video"
        );
        assert_eq!(
            attachment_preview_key(&[att("audio/webm")]),
            "home-dash-preview-voice"
        );
        assert_eq!(
            attachment_preview_key(&[att("application/pdf")]),
            "home-dash-preview-file"
        );
    }

    #[test]
    fn preview_key_uses_first_attachment_and_falls_back_when_empty() {
        // Precedence follows the timeline's render order: the message leads with
        // its first attachment, so that is what the label reflects.
        assert_eq!(
            attachment_preview_key(&[att("image/png"), att("application/zip")]),
            "home-dash-preview-image"
        );
        // A bodyless message with no loadable attachments gets the generic label.
        assert_eq!(attachment_preview_key(&[]), "home-dash-preview-attachment");
    }
}
