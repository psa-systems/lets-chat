use askama::Template;

use crate::models::custom_emoji::CustomEmoji;
use crate::models::enclave::{Enclave, EnclaveInvitation, EnclaveRole};
use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row in the enclave landing/settings members list. Carries a pre-
/// resolved `label` (display_name when set, otherwise `@username`) so the
/// template never has to render the opaque user_id.
pub struct EnclaveMemberView {
    pub user_id: String,
    pub label: String,
    pub role: EnclaveRole,
}

#[derive(Template)]
#[template(path = "enclave/page.html")]
pub struct EnclavePage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
    pub members: &'a [EnclaveMemberView],
    pub rooms: &'a [Room],
    pub can_manage: bool,
    pub flash_error: Option<&'a str>,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

pub struct EnclaveGroupView {
    pub id: i64,
    pub name: String,
    pub member_count: i64,
    pub member_labels: Vec<String>,
}

#[derive(Template)]
#[template(path = "enclave/settings.html")]
pub struct EnclaveSettingsPage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
    pub members: &'a [EnclaveMemberView],
    pub groups: &'a [EnclaveGroupView],
    pub emojis: &'a [CustomEmoji],
    pub can_delete: bool,
    pub flash_error: Option<&'a str>,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "enclave/discover.html")]
pub struct DiscoverPage<'a> {
    pub user: &'a User,
    pub enclaves: &'a [Enclave],
    pub flash_error: Option<&'a str>,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

/// One candidate row in the enclave-invite typeahead. Carries the same
/// fields the user-search popover uses (so we can reuse the avatar partial
/// without spreading raw user_ids into the template) plus a per-row state
/// flag: already-a-member rows render a disabled pill instead of an Invite
/// button, and already-invited rows render "Invited" instead. The route
/// resolves these flags so the template stays presentation-only.
pub struct EnclaveInviteCandidate {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub state: EnclaveInviteCandidateState,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EnclaveInviteCandidateState {
    /// Not yet a member or invitee; the Invite button is active.
    Invitable,
    /// Already a confirmed member of the enclave.
    AlreadyMember,
    /// Has an outstanding invitation that has not yet been accepted/declined.
    AlreadyInvited,
    /// The caller is looking at themselves. We don't let users self-invite.
    Self_,
}

impl EnclaveInviteCandidate {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
    pub fn is_invitable(&self) -> bool {
        matches!(self.state, EnclaveInviteCandidateState::Invitable)
    }
    pub fn is_member(&self) -> bool {
        matches!(self.state, EnclaveInviteCandidateState::AlreadyMember)
    }
    pub fn is_invited(&self) -> bool {
        matches!(self.state, EnclaveInviteCandidateState::AlreadyInvited)
    }
    pub fn is_self(&self) -> bool {
        matches!(self.state, EnclaveInviteCandidateState::Self_)
    }
}

#[derive(Template)]
#[template(path = "enclave/invite_search.html")]
pub struct EnclaveInviteSearchFragment<'a> {
    pub enclave_id: i64,
    pub query: &'a str,
    pub results: &'a [EnclaveInviteCandidate],
}

/// Per-row response returned by `POST /enclave/{id}/invite`. The typeahead
/// row swaps itself with this fragment via `hx-swap="outerHTML"`. `ok` is
/// true on a successful invite (renders "Invited"), false on a validation
/// error (renders an inline red message).
#[derive(Template)]
#[template(path = "enclave/invite_row_result.html")]
pub struct EnclaveInviteRowResult<'a> {
    pub ok: bool,
    pub message: &'a str,
}

#[derive(Template)]
#[template(path = "invitations/page.html")]
pub struct InvitationsPage<'a> {
    pub user: &'a User,
    pub invitations: &'a [(EnclaveInvitation, Enclave)],
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}
