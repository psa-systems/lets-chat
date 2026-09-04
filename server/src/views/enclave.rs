#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::custom_emoji::CustomEmoji;
use crate::models::enclave::{Enclave, EnclaveRole};
use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row in the enclave settings members list. Carries a pre-resolved
/// `label` (display_name when set, otherwise `@username`) so the template
/// never has to render the opaque user_id.
pub struct EnclaveMemberView {
    pub user_id: String,
    pub label: String,
    pub role: EnclaveRole,
    /// LC-551: `"new"` or `"trusted"`. Owners/admins are always effectively
    /// trusted; the template only surfaces the pill/control for `member` rows.
    pub trust: String,
}

/// LC-516: one selectable bot in the "add a bot" picker on the members panel.
/// Only site bots that are not already members of the enclave are listed.
pub struct EnclaveBotOption {
    pub id: String,
    pub label: String,
}

/// LC-336: empty-state placeholder shown by `get_landing` only when the enclave
/// has no openable rooms. The full landing menu was removed; create-chat now
/// lives on the sidebar `+` and member management in settings.
#[derive(Template)]
#[template(path = "enclave/page.html")]
pub struct EnclavePage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
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

/// One row in the per-group member-add typeahead. Mirrors
/// `EnclaveInviteCandidate` but with group-relevant state flags:
/// already-in-group renders an "Added" pill, not-in-enclave renders a
/// disabled "Not in enclave" pill, the caller can add themselves so
/// there is no Self_ variant.
pub struct GroupMemberCandidate {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub state: GroupMemberCandidateState,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GroupMemberCandidateState {
    Addable,
    AlreadyInGroup,
    NotInEnclave,
}

impl GroupMemberCandidate {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
    pub fn is_addable(&self) -> bool {
        matches!(self.state, GroupMemberCandidateState::Addable)
    }
    pub fn is_in_group(&self) -> bool {
        matches!(self.state, GroupMemberCandidateState::AlreadyInGroup)
    }
    pub fn is_outside_enclave(&self) -> bool {
        matches!(self.state, GroupMemberCandidateState::NotInEnclave)
    }
}

#[derive(Template)]
#[template(path = "enclave/group_member_search.html")]
pub struct GroupMemberSearchFragment<'a> {
    pub enclave_id: i64,
    pub group_id: i64,
    pub query: &'a str,
    pub results: &'a [GroupMemberCandidate],
}

/// Per-row response returned by `POST /enclave/{id}/groups/{gid}/members`
/// when the request originates from the typeahead. The row swaps itself
/// via `hx-swap="outerHTML"`. `ok` true on add (renders "Added"), false
/// on validation error (renders inline red message).
#[derive(Template)]
#[template(path = "enclave/group_member_row_result.html")]
pub struct GroupMemberRowResult<'a> {
    pub ok: bool,
    pub message: &'a str,
}

/// LC-340: one row of the enclave ban-list as rendered in settings. `label` is
/// the resolved display name; `user_id` keys the unban form.
pub struct EnclaveBanView {
    pub user_id: String,
    pub label: String,
    pub reason: Option<String>,
    pub banned_at: String,
}

#[derive(Template)]
#[template(path = "enclave/settings.html")]
pub struct EnclaveSettingsPage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
    pub members: &'a [EnclaveMemberView],
    /// LC-516: site bots not already in this enclave, for the add-bot picker.
    pub bots: &'a [EnclaveBotOption],
    pub bans: &'a [EnclaveBanView],
    pub groups: &'a [EnclaveGroupView],
    pub emojis: &'a [CustomEmoji],
    pub can_delete: bool,
    /// LC-542: whether this enclave has a configured icon (branding logo,
    /// resolved with the global fallback), driving the inline icon preview vs
    /// the neutral placeholder on the settings page.
    pub has_icon: bool,
    pub flash_error: Option<&'a str>,
    /// LC-463: localized success message, shown as a toast on load.
    pub flash_ok: Option<&'a str>,
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
#[template(path = "enclave/branding.html")]
pub struct EnclaveBrandingPage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub primary_color: String,
    pub accent_color: String,
    pub login_heading: String,
    pub login_body: String,
    pub has_logo: bool,
    pub saved: bool,
    pub error: Option<String>,
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

/// LC-767: the invite search hosted in the shared `#thread-panel` drawer, so
/// the invite control is reachable from the room header and the member panel
/// (where people look for it) and not only the sidebar foot. It only wraps a
/// drawer shell around the existing invite endpoints; the search input targets
/// `GET /enclave/{id}/invite/search` and each result row posts to
/// `POST /enclave/{id}/invite`, exactly as the settings page does.
#[derive(Template)]
#[template(path = "enclave/invite_panel.html")]
pub struct EnclaveInvitePanel {
    pub enclave_id: i64,
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

/// LC-772: one pending-invitation card, fully resolved by the route so the
/// template never touches business logic or raw ids. Carries the enclave
/// context the invitee decides on (name, description, member count, optional
/// logo) and the inviter resolved to a friendly name + avatar. Shared by the
/// full invitations page and the live OOB fragment so both render identically.
pub struct InvitationCard {
    pub id: i64,
    pub enclave_id: i64,
    pub enclave_name: String,
    pub enclave_description: Option<String>,
    pub member_count: i64,
    /// URL of the enclave's branding logo, or None to fall back to a monogram.
    pub logo_url: Option<String>,
    pub inviter_id: String,
    /// Inviter display name / handle, already resolved (never a raw UUID).
    pub inviter_name: String,
    pub inviter_avatar_ext: Option<String>,
}

impl InvitationCard {
    /// Uppercase first character of the enclave name, for the monogram tile
    /// shown when the enclave has no logo.
    pub fn enclave_initial(&self) -> String {
        first_initial(&self.enclave_name)
    }

    /// Uppercase first character of the inviter name, for the avatar fallback.
    pub fn inviter_initial(&self) -> String {
        first_initial(&self.inviter_name)
    }

    /// Member count by value, so the `tn` plural filter receives an `i64`
    /// rather than the `&i64` a bare field access yields in the template.
    pub fn members(&self) -> i64 {
        self.member_count
    }
}

fn first_initial(s: &str) -> String {
    s.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

/// LC-161: OOB fragment swapping the live `#lc-invitations` region over the
/// WebSocket when a user's invitation set changes.
#[derive(Template)]
#[template(path = "ws/invitations_live.html")]
pub struct InvitationsLiveFragment<'a> {
    pub invitations: &'a [InvitationCard],
}

/// LC-172: OOB fragment swapping the live `#lc-enclave-settings-members` region
/// on the enclave settings page when membership/roles change. Carries the
/// role-toggle / kick / transfer controls gated on `can_delete`, so it is
/// rendered per recipient. Shares `enclave/settings_members_items.html` with
/// `EnclaveSettingsPage`.
#[derive(Template)]
#[template(path = "ws/enclave_settings_members_live.html")]
pub struct EnclaveSettingsMembersLiveFragment<'a> {
    pub enclave: &'a Enclave,
    pub members: &'a [EnclaveMemberView],
    pub can_delete: bool,
}

#[derive(Template)]
#[template(path = "invitations/page.html")]
pub struct InvitationsPage<'a> {
    pub user: &'a User,
    pub invitations: &'a [InvitationCard],
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    /// LC-864: the environment this account (and therefore any invitation shown
    /// here) is provisioned against, or None in production. Stated on the page
    /// so a recipient knows whether the invite is on staging or production.
    pub environment: Option<&'a str>,
}
