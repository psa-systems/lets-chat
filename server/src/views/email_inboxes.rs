//! LC-77 admin views: per-room email-ingress inbox management.
//! Mirrors `crate::views::webhooks` field-for-field; the only template
//! shape divergence is the help text + the "address shown once" affordance
//! (full `<token>@<domain>` instead of LC-74's `<base>/webhook/<secret>`).

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One inbox row on the room's email-inbox page (no secret material).
pub struct EmailInboxRowView {
    pub id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

#[derive(Template)]
#[template(path = "room/email_inboxes.html")]
pub struct RoomEmailInboxesPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    /// True when the server has a secret key (required to hash inbox
    /// secrets) AND the ingress domain is configured. When false, creation
    /// is disabled and a help banner names the missing piece.
    pub available: bool,
    /// `Some` when secret_key is set but ingress_domain is not, so the UI
    /// can tell the operator exactly which setting they still owe.
    pub missing_setting: Option<&'static str>,
    pub inboxes: &'a [EmailInboxRowView],
    /// Full inbox address for a just-created inbox, shown exactly once.
    pub new_address: Option<String>,
    pub error: Option<String>,
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
