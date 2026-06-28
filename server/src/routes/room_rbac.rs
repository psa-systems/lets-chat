//! LC-84: per-room moderator override admin surface.
//!
//! GET `/room/{id}/moderators` renders the page; POST grants an override;
//! DELETE revokes one. Mutations are gated on
//! `perms::room_can_manage_overrides` (site admin / enclave owner /
//! enclave admin) and write a `mod_actions` audit row so the action is
//! discoverable in the moderation log.
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::i18n::translate_current;
use crate::models::User;
use crate::perms::room_can_manage_overrides;
use crate::state::AppState;
use crate::views::room_moderators::{RoomModeratorRow, RoomModeratorsPage, RoomOverrideEntry};
use crate::views::settings::SettingsFeedback;
use crate::views::{html, Html};

/// LC-454: true when the request came from htmx, so a mutating handler returns a
/// small status fragment (the shared `SettingsFeedback`) instead of a full-page
/// redirect. Mirrors `routes::settings::is_hx`.
fn is_hx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

/// LC-454: `GET /room/{id}/moderators` -> `/room/{id}/manage`. The page was
/// renamed + relocated; this keeps old links / bookmarks working. The mutating
/// action routes (POST grant, DELETE revoke, posting-policy, retention) keep
/// their original URLs.
pub async fn redirect_to_manage(Path(room_id): Path<i64>) -> Redirect {
    Redirect::to(&format!("/room/{room_id}/manage"))
}

#[derive(Deserialize)]
pub struct GrantForm {
    pub user_id: String,
    /// `"moderator"` or `"admin"`. Anything else is rejected with 400.
    pub role: String,
}

/// Authorize the caller. The room's enclave determines who can manage
/// overrides; rooms with no enclave (legacy) fall back to site-admin only.
/// `pub(crate)` so sibling route modules with the same gating (retention,
/// future room-admin settings) can share the check.
pub(crate) async fn require_can_manage(
    state: &AppState,
    user: &User,
    room_id: i64,
) -> Result<(), AppError> {
    let enclave_id = super::enclave_for_room(state, room_id).await?;
    let enclave_role = if let Some(eid) = enclave_id {
        db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role)
    } else {
        None
    };
    if !room_can_manage_overrides(enclave_role, &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Resolve `display_name` if non-empty, else `@username`.
fn label_for(username: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => format!("@{username}"),
    }
}

/// GET /room/{id}/moderators
pub async fn get_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_can_manage(&state, &user, room_id).await?;

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let raw_overrides = db::room_rbac::list_for_room(&state.chat, room_id).await?;
    let mut overrides: Vec<RoomOverrideEntry> = Vec::with_capacity(raw_overrides.len());
    for ovr in raw_overrides {
        let target = db::auth::find_user_by_id(&state.auth, &ovr.user_id).await?;
        let actor = db::auth::find_user_by_id(&state.auth, &ovr.assigned_by).await?;
        overrides.push(RoomOverrideEntry {
            user_id: ovr.user_id,
            user_label: target
                .as_ref()
                .map(|r| label_for(&r.username, r.display_name.as_deref()))
                .unwrap_or_else(|| "(unknown)".to_string()),
            role: ovr.role,
            assigned_by_label: actor
                .as_ref()
                .map(|r| label_for(&r.username, r.display_name.as_deref()))
                .unwrap_or_else(|| "(unknown)".to_string()),
            assigned_at: ovr.assigned_at,
        });
    }

    // Candidate list: every member of the room's enclave who does NOT
    // already have an override. The grant form's <select> renders these
    // rows; revoke buttons handle the override list above.
    let mut candidates: Vec<RoomModeratorRow> = Vec::new();
    let enclave_id_for_chrome = super::enclave_for_room(&state, room_id).await?;
    if let Some(eid) = enclave_id_for_chrome {
        let members = db::enclave::list_members(&state.chat, eid).await?;
        let has_override: std::collections::HashSet<String> =
            overrides.iter().map(|o| o.user_id.clone()).collect();
        for m in members {
            if has_override.contains(&m.user_id) {
                continue;
            }
            if m.user_id == user.id {
                continue;
            }
            let rec = db::auth::find_user_by_id(&state.auth, &m.user_id).await?;
            let label = rec
                .as_ref()
                .map(|r| label_for(&r.username, r.display_name.as_deref()))
                .unwrap_or_else(|| "(unknown)".to_string());
            candidates.push(RoomModeratorRow {
                user_id: m.user_id,
                label,
            });
        }
    }
    candidates.sort_by(|a, b| a.label.cmp(&b.label));

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, enclave_id_for_chrome).await?;

    let retention_days = db::chat::get_room_retention_days(&state.chat, room_id).await?;
    let broadcast_policy = db::chat::get_room_broadcast_policy(&state.chat, room_id).await?;

    html(&RoomModeratorsPage {
        user: &user,
        room: &room,
        enclave_id: enclave_id_for_chrome,
        overrides: &overrides,
        candidates: &candidates,
        posting_policy: &room.posting_allowed_for,
        broadcast_policy: &broadcast_policy,
        retention_days,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}

/// POST /room/{id}/moderators  body: user_id=..&role=moderator
pub async fn post_grant(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<GrantForm>,
) -> Result<impl IntoResponse, AppError> {
    require_can_manage(&state, &user, room_id).await?;
    let role = form.role.trim().to_ascii_lowercase();
    if role != "moderator" && role != "admin" {
        return Err(AppError::BadRequest(
            "role must be 'moderator' or 'admin'".into(),
        ));
    }
    // Target must be a member of the room's enclave; otherwise an
    // override grants powers in a room they cannot see.
    let enclave_id = super::enclave_for_room(&state, room_id).await?;
    if let Some(eid) = enclave_id {
        let m = db::enclave::get_membership(&state.chat, eid, &form.user_id).await?;
        if m.is_none() {
            return Err(AppError::BadRequest(
                "target user is not a member of this room's enclave".into(),
            ));
        }
    }
    db::room_rbac::upsert(&state.chat, room_id, &form.user_id, &role, &user.id).await?;
    let metadata = format!(r#"{{"role":"{role}"}}"#);
    db::moderation::log_mod_action(
        &state.chat,
        "room_role_grant",
        &form.user_id,
        &user.id,
        None,
        Some(room_id),
        Some(&metadata),
    )
    .await?;
    Ok(Redirect::to(&format!("/room/{room_id}/manage")))
}

#[derive(Deserialize)]
pub struct PostingPolicyForm {
    /// One of `"all"`, `"moderators_only"`, `"admins_only"`. Anything
    /// else 400s.
    pub policy: String,
}

/// POST /room/{id}/posting-policy
///
/// LC-85: flip a room into / out of read-only or moderators-only mode.
/// Authorized to the same set as override grant/revoke (`enclave_can_
/// manage`). Writes a `mod_actions` audit row so policy changes are
/// discoverable in the moderation log.
pub async fn post_posting_policy(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PostingPolicyForm>,
) -> Result<Response, AppError> {
    require_can_manage(&state, &user, room_id).await?;
    let policy = form.policy.trim();
    if !matches!(policy, "all" | "moderators_only" | "admins_only") {
        return Err(AppError::BadRequest(
            "policy must be one of: all, moderators_only, admins_only".into(),
        ));
    }
    let n = db::chat::set_room_posting_policy(&state.chat, room_id, policy).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    let metadata = format!(r#"{{"posting_allowed_for":"{policy}"}}"#);
    db::moderation::log_mod_action(
        &state.chat,
        "room_posting_policy",
        "",
        &user.id,
        None,
        Some(room_id),
        Some(&metadata),
    )
    .await?;
    // LC-454: htmx submit gets the shared status fragment + toast; a no-JS
    // submit falls back to a full-page redirect (progressive enhancement).
    if is_hx(&headers) {
        return Ok(html(&SettingsFeedback::ok(translate_current(
            "room-policy-saved",
        )))?
        .into_response());
    }
    Ok(Redirect::to(&format!("/room/{room_id}/manage")).into_response())
}

/// POST /room/{id}/broadcast-policy
///
/// LC-476: set who may use `@here`/`@channel` in this room (the politeness
/// gate). Same authorization + audit + dual-mode response shape as
/// `post_posting_policy`; reuses `PostingPolicyForm` (identical `policy` field).
pub async fn post_broadcast_policy(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PostingPolicyForm>,
) -> Result<Response, AppError> {
    require_can_manage(&state, &user, room_id).await?;
    let policy = form.policy.trim();
    if !matches!(policy, "all" | "moderators_only" | "admins_only") {
        return Err(AppError::BadRequest(
            "policy must be one of: all, moderators_only, admins_only".into(),
        ));
    }
    let n = db::chat::set_room_broadcast_policy(&state.chat, room_id, policy).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    let metadata = format!(r#"{{"broadcast_allowed_for":"{policy}"}}"#);
    db::moderation::log_mod_action(
        &state.chat,
        "room_broadcast_policy",
        "",
        &user.id,
        None,
        Some(room_id),
        Some(&metadata),
    )
    .await?;
    if is_hx(&headers) {
        return Ok(html(&SettingsFeedback::ok(translate_current(
            "room-policy-saved",
        )))?
        .into_response());
    }
    Ok(Redirect::to(&format!("/room/{room_id}/manage")).into_response())
}

/// DELETE /room/{id}/moderators/{user_id}
pub async fn delete_revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, target)): Path<(i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    require_can_manage(&state, &user, room_id).await?;
    let n = db::room_rbac::delete(&state.chat, room_id, &target).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    db::moderation::log_mod_action(
        &state.chat,
        "room_role_revoke",
        &target,
        &user.id,
        None,
        Some(room_id),
        None,
    )
    .await?;
    Ok(Redirect::to(&format!("/room/{room_id}/manage")))
}
