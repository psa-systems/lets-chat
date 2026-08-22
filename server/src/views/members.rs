//! LC-683: the room members panel, rendered into the shared `#thread-panel`
//! slot (the same drawer the thread view and catch-me-up summary use). Opened
//! from the header member-avatar cluster instead of navigating to the room
//! Info page. Reuses `partials/avatar.html` for the live presence dot and the
//! profile hovercard for per-member profile / DM actions.

use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;

/// A role badge shown next to an elevated member's name. Label is resolved to
/// the viewer's locale in the handler (so the template stays comparison-free);
/// `class` carries the Tailwind color classes for that role.
pub struct MemberBadge {
    pub label: String,
    pub class: &'static str,
}

/// One row in the members roster.
pub struct MemberRow {
    pub user_id: String,
    /// Display label (display name, else `@username`).
    pub label: String,
    pub username: String,
    pub avatar_ext: Option<String>,
    /// Effective presence string (`online`/`idle`/`dnd`/`offline`) resolved via
    /// `routes::effective_status`, so the dot matches every other avatar surface.
    pub status: String,
    pub custom_status: Option<String>,
    /// Role badge for an elevated member (owner/admin/mod), or `None` for a
    /// plain member (no badge, to keep the list quiet).
    pub badge: Option<MemberBadge>,
    /// Lowercased "label @username" haystack for the client-side filter.
    pub filter_key: String,
    /// LC-689: the viewer's own row. Rendered without a DM link (you cannot DM
    /// yourself - `/dm/{self}` 404s) and marked "You".
    pub is_self: bool,
}

/// The members drawer. Occupies the shared `#thread-panel` slot, so it reuses
/// the thread panel's close affordance (`DELETE /thread-panel`).
#[derive(Template)]
#[template(path = "room/members_panel.html")]
pub struct MembersPanel {
    pub room_id: i64,
    pub member_count: usize,
    pub members: Vec<MemberRow>,
    /// Whether to show the gated "Manage members" footer link to
    /// `/room/{id}/manage` (viewer can manage role overrides).
    pub can_manage: bool,
    /// LC-767: the room's enclave, when it has one. Drives the "Invite people"
    /// footer link (which opens `/enclave/{id}/invite/panel`). `None` for a DM
    /// or any room without an enclave, where there is nobody to invite.
    pub enclave_id: Option<i64>,
}
