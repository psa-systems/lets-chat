#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::room_automations::AutomationRow;

/// LC-677: a self-re-rendering on/off settings toggle. Rendered inline (via
/// `{% include %}` on the room Manage and enclave Settings pages) and returned
/// by the toggle handlers so a click updates the switch, label, and hidden
/// next-value in place, no reload. LC-747 replaced the hardcoded
/// `/room/{id}/{field}` post target with a caller-provided `action` plus the
/// form field `name`, so any boolean setting can use it. `status` is empty on
/// the page render, "Saved" after a toggle. See
/// `partials/settings_toggle.html`.
#[derive(Template)]
#[template(path = "partials/settings_toggle.html")]
pub struct SettingsToggleFragment {
    pub action: String,
    pub name: &'static str,
    pub enabled: bool,
    pub aria_label: String,
    pub on_label: String,
    pub on_text: String,
    pub off_label: String,
    pub off_text: String,
    pub status: String,
}

/// One row in the existing-overrides table.
pub struct RoomOverrideEntry {
    pub user_id: String,
    pub user_label: String,
    pub role: String,
    pub assigned_by_label: String,
    pub assigned_at: String,
}

/// One option in the grant <select> (room-enclave member who does NOT
/// already have an override).
pub struct RoomModeratorRow {
    pub user_id: String,
    pub label: String,
}

/// LC-454: the room "Manage" page (renamed from "Moderators"). Reached at
/// `/room/{id}/manage` (`/moderators` 302-redirects here for back-compat).
/// Same `room_can_manage_overrides` gate; groups posting policy, integrations,
/// roles/overrides, message retention, and the delete-room danger zone.
#[derive(Template)]
#[template(path = "room/manage.html")]
pub struct RoomModeratorsPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    /// LC-454: the room's enclave id, used to build the delete-room POST target
    /// `/enclave/{enclave_id}/rooms/{room_id}/delete`. `None` for DMs / rooms
    /// with no enclave (the danger zone is hidden then).
    pub enclave_id: Option<i64>,
    pub overrides: &'a [RoomOverrideEntry],
    pub candidates: &'a [RoomModeratorRow],
    /// LC-85: current `posting_allowed_for` for this room. Drives the
    /// "Posting policy" dropdown's selected option.
    pub posting_policy: &'a str,
    /// LC-476: current `broadcast_allowed_for` for this room. Drives the
    /// "Broadcast mentions" dropdown's selected option.
    pub broadcast_policy: &'a str,
    /// LC-534: current `rooms.slowmode_seconds` (0 = off).
    pub slowmode_seconds: u32,
    /// Current `rooms.retention_days`. `None` = retention disabled
    /// (default); `Some(N)` = messages older than N days are deleted
    /// on the next sweep. The Retention section in the moderators
    /// template uses this to pre-fill the form and label the
    /// current state.
    pub retention_days: Option<i64>,
    /// LC-492: whether the in-channel AI assistant (`/ask`) is enabled here.
    pub assistant_enabled: bool,
    /// LC-492: whether the operator has configured an LLM at all. When false the
    /// toggle still saves but a hint explains the assistant won't function yet.
    pub assistant_available: bool,
    /// LC-665: whether the scheduled AI activity digest is enabled here. Shares
    /// `assistant_available` for the "no LLM configured" hint.
    pub digest_enabled: bool,
    /// LC-494: whether "stage" mode is enabled for this room.
    pub stage_enabled: bool,
    /// LC-495: this room's workflow-automation rules (shown via the included
    /// `partials/room_automations.html`; empty for DMs, where the section is
    /// hidden).
    pub automations: &'a [AutomationRow],
    pub sidebar_categories: &'a [SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}
