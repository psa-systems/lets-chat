//! Shared delivery-gate checks for scheduled-send. Both
//! [`crate::routes::scheduled`] (schedule time) and
//! [`crate::scheduled::dispatcher`] (delivery time) must agree on this
//! set, or a user could schedule a message they cannot send (or, worse,
//! the dispatcher could ship a message the user is no longer allowed to
//! send). Keeping the gates in one helper prevents that drift.
//!
//! If you change the gates here, audit
//! [`crate::routes::room::post_message`] for the live-send path. Task 1
//! intentionally did not pull the live path into a shared helper (its
//! refactor scope was strictly the post-insert tail), so the live path
//! has its own copy of these checks that must be updated by hand.

use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;

/// Reason a delivery attempt is being refused. The string forms are
/// load-bearing: they are persisted to `scheduled_messages.failed_reason`
/// and surfaced in the `/scheduled` "Recently dropped" section. Renaming
/// a variant's `as_str()` value is a UI-visible change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    Banned,
    Muted,
    NoRoomAccess,
    PolicyDenied,
    DmBlocked,
    LinkBlocked,
}

impl DenyReason {
    pub fn as_str(self) -> &'static str {
        match self {
            DenyReason::Banned => "author is banned",
            DenyReason::Muted => "author is muted",
            DenyReason::NoRoomAccess => "author lost room access",
            DenyReason::PolicyDenied => "author cannot post in this room",
            DenyReason::DmBlocked => "DM peer blocked author",
            DenyReason::LinkBlocked => "body contains a blocked link",
        }
    }
}

/// Run the gates that apply both at schedule time and at delivery time.
/// `Ok(None)` means "all gates pass, proceed"; `Ok(Some(reason))` means
/// "refuse the send, surface the reason"; `Err` is an internal DB error.
///
/// The deliberate omission is the rate-limit check: by ticket decision,
/// rate-limit is enforced at schedule time (so a user can't queue 1000
/// scheduled messages in a second) but NOT re-checked at delivery (the
/// user already paid for the slot, dispatching shouldn't second-guess
/// them). The caller adds the rate-limit check itself at schedule time.
pub async fn check_delivery_gates(
    state: &AppState,
    user: &User,
    room: &Room,
    body: &str,
) -> Result<Option<DenyReason>, AppError> {
    if user.is_banned {
        return Ok(Some(DenyReason::Banned));
    }
    if user.is_muted {
        return Ok(Some(DenyReason::Muted));
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room.id, &user.id, is_admin).await? {
        return Ok(Some(DenyReason::NoRoomAccess));
    }
    if !crate::routes::room::can_post_with_policy(state, user, room.id, &room.posting_allowed_for)
        .await?
    {
        return Ok(Some(DenyReason::PolicyDenied));
    }
    if room.room_type == "dm" {
        let members = db::chat::list_room_member_ids(&state.chat, room.id).await?;
        if let Some(peer_id) = members.iter().find(|id| **id != user.id) {
            if db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await? {
                return Ok(Some(DenyReason::DmBlocked));
            }
        }
    }
    let link_filter_on = db::settings::get_setting(&state.settings, "link_filter_enabled")
        .await?
        .as_deref()
        == Some("true");
    if link_filter_on {
        let hosts = crate::links::extract_hosts(body);
        if let Some((rule, _host)) = db::anti_spam::find_match(&state.chat, &hosts).await? {
            if rule.action == db::anti_spam::FilterAction::Block {
                return Ok(Some(DenyReason::LinkBlocked));
            }
        }
    }
    Ok(None)
}
