//! LC-683: the room members panel.
//!
//! Opened from the header member-avatar cluster (which previously navigated to
//! the room Info page, a dead end with no member list). Renders the roster into
//! the shared `#thread-panel` slot, so it reuses the thread panel's close
//! affordance (`DELETE /thread-panel`) and its one-drawer-at-a-time behavior.
//!
//! Reuse over rebuild: effective member ids come from the shared
//! `routes::effective_member_ids` helper (so the header count and the panel
//! never diverge), presence from `routes::effective_status`, avatars from
//! `partials/avatar.html`, and per-member profile / DM from the existing
//! hovercard. Role badges merge enclave membership with per-room overrides.
//! Management stays gated and links out to `/room/{id}/manage` rather than
//! reimplementing promote/kick inline.

use std::collections::HashMap;

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::enclave::EnclaveRole;
use crate::state::AppState;
use crate::views::members::{MemberBadge, MemberRow, MembersPanel};
use crate::views::{html, Html};

/// Badge rank for sorting + for picking the strongest of two role sources.
/// `owner > admin > moderator > (plain member)`.
fn rank(role: Option<&str>) -> u8 {
    match role {
        Some("owner") => 3,
        Some("admin") => 2,
        Some("moderator") => 1,
        _ => 0,
    }
}

/// Resolve the badge to show for a member from the two role sources: enclave
/// membership (owner/admin; a plain enclave member gets no badge) and any
/// per-room override (admin/moderator). The stronger of the two wins.
fn badge_for(enclave: Option<&EnclaveRole>, rbac: Option<&str>) -> Option<&'static str> {
    let e: Option<&'static str> = match enclave {
        Some(EnclaveRole::Owner) => Some("owner"),
        Some(EnclaveRole::Admin) => Some("admin"),
        _ => None,
    };
    let r: Option<&'static str> = match rbac {
        Some("admin") => Some("admin"),
        Some("moderator") => Some("moderator"),
        _ => None,
    };
    if rank(e) >= rank(r) {
        e
    } else {
        r
    }
}

/// Turn a resolved role key into a locale-aware badge (label + Tailwind color).
/// `None` for a plain member, so the row shows no badge.
fn badge_view(role: Option<&'static str>) -> Option<MemberBadge> {
    let (key, class) = match role? {
        "owner" => ("members-role-owner", "border border-accent text-accent"),
        "admin" => ("members-role-admin", "border border-warning text-warning"),
        "moderator" => (
            "members-role-moderator",
            "border border-border text-content-muted",
        ),
        _ => return None,
    };
    Some(MemberBadge {
        label: crate::i18n::translate_current(key),
        class,
    })
}

/// GET /room/{room_id}/members - the members roster drawer, rendered into the
/// shared `#thread-panel` slot.
pub async fn get_members_panel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let enclave_id = super::enclave_for_room(&state, room_id).await?;
    let member_ids =
        super::effective_member_ids(&state, room_id, &room.room_type, enclave_id).await?;

    // Role maps, one bulk query each. Enclave roles (owner/admin/member) and any
    // per-room override (admin/moderator) merge into a single badge per member.
    let mut enclave_roles: HashMap<String, EnclaveRole> = HashMap::new();
    if let Some(eid) = enclave_id {
        for m in db::enclave::list_members(&state.chat, eid).await? {
            enclave_roles.insert(m.user_id, m.role);
        }
    }
    let mut rbac: HashMap<String, String> = HashMap::new();
    for o in db::room_rbac::list_for_room(&state.chat, room_id).await? {
        rbac.insert(o.user_id, o.role);
    }

    // Build (rank, row) pairs so the roster can sort elevated members first
    // without carrying the raw role string into the view.
    let mut ranked: Vec<(u8, MemberRow)> = Vec::with_capacity(member_ids.len());
    for id in &member_ids {
        let Some(rec) = db::auth::find_user_by_id(&state.auth, id).await? else {
            continue;
        };
        // A banned user is not really "in" the room; skip, as the mention and
        // user-search surfaces do.
        if rec.is_banned {
            continue;
        }
        let label = match rec.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n.to_string(),
            _ => format!("@{}", rec.username),
        };
        // Presence resolved the one way every avatar surface agrees on.
        let status = super::effective_status(&state, &rec.id, &rec.status);
        let role = badge_for(enclave_roles.get(id), rbac.get(id).map(String::as_str));
        let filter_key = format!("{} @{}", label.to_lowercase(), rec.username.to_lowercase());
        let is_self = rec.id == user.id;
        ranked.push((
            rank(role),
            MemberRow {
                user_id: rec.id,
                label,
                username: rec.username,
                avatar_ext: rec.avatar_ext,
                status,
                custom_status: rec.custom_status,
                badge: badge_view(role),
                filter_key,
                is_self,
            },
        ));
    }

    // Elevated roles first (owner > admin > mod), then alphabetical by label.
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase()))
    });
    let members: Vec<MemberRow> = ranked.into_iter().map(|(_, row)| row).collect();
    let member_count = members.len();

    // Gate the "Manage members" footer link on the same permission the header
    // manage icon uses (`room_can_manage_overrides`).
    let enclave_role = if let Some(eid) = enclave_id {
        db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role)
    } else {
        None
    };
    let can_manage = crate::perms::room_can_manage_overrides(enclave_role, &user.role);

    html(&MembersPanel {
        room_id,
        member_count,
        members,
        can_manage,
    })
}

#[cfg(test)]
mod tests {
    use super::{badge_for, rank};
    use crate::models::enclave::EnclaveRole;

    #[test]
    fn badge_picks_the_stronger_role_source() {
        // Enclave owner outranks any override.
        assert_eq!(
            badge_for(Some(&EnclaveRole::Owner), Some("admin")),
            Some("owner")
        );
        // A per-room moderator override on a plain enclave member shows the mod badge.
        assert_eq!(
            badge_for(Some(&EnclaveRole::Member), Some("moderator")),
            Some("moderator")
        );
        // A per-room admin override outranks nothing-from-enclave.
        assert_eq!(badge_for(None, Some("admin")), Some("admin"));
        // Plain member, no override: no badge.
        assert_eq!(badge_for(Some(&EnclaveRole::Member), None), None);
        assert_eq!(badge_for(None, None), None);
    }

    #[test]
    fn rank_orders_roles() {
        assert!(rank(Some("owner")) > rank(Some("admin")));
        assert!(rank(Some("admin")) > rank(Some("moderator")));
        assert!(rank(Some("moderator")) > rank(None));
        assert_eq!(rank(Some("bogus")), 0);
    }
}
