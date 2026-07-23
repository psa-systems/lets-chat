#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// LC-470: public marketing landing page shown at `/` to logged-out visitors
/// (authenticated users go straight to the chat home). Mirrors the Bunyip /
/// Mokosh landing pattern: hero with the messenger-bird mascot, a features
/// grid, and a "Sign in with Bunyip" call to action.
#[derive(Template)]
#[template(path = "landing.html")]
pub struct LandingPage<'a> {
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
    /// Host portion of the configured Bunyip issuer, shown in the
    /// "you'll be redirected to ..." note (same as the login page).
    pub bunyip_issuer_host: &'a str,
}

#[derive(Template)]
#[template(path = "home/welcome.html")]
pub struct WelcomePage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub flash_error: Option<&'a str>,
    /// LC-372: count of pending enclave invitations, drives the count badge on
    /// the welcome empty-state "View invitations" quick action (hidden when 0).
    pub pending_invites: usize,
    /// LC-575: when true, render the Home dashboard card grid (the user has at
    /// least one accessible room or DM). When false the onboarding welcome /
    /// empty-state renders instead, exactly as before.
    pub show_dashboard: bool,
    /// LC-575: dashboard cards. Each is capped server-side; empty slices render
    /// an "all caught up" state per card. See `routes::home::get_home`.
    pub catch_up: &'a [CatchUpRow],
    pub mentions: &'a [MentionRow],
    pub threads: &'a [ThreadRow],
    pub dms: &'a [DmRow],
    pub drafts: &'a [DraftRow],
}

/// LC-575: one channel-with-unread row on the dashboard "Catch up" card.
pub struct CatchUpRow {
    pub room_id: i64,
    pub name: String,
    /// One-line preview of the newest unread message.
    pub preview: String,
    pub unread: i64,
}

/// LC-575: one "you were mentioned" summary row (per room) on the Mentions card.
pub struct MentionRow {
    pub room_id: i64,
    pub name: String,
    pub count: i64,
}

/// LC-575: one followed-thread row on the Threads card. Links into the room
/// anchored at the thread's root message (`/room/{room_id}#msg-{parent_id}`).
pub struct ThreadRow {
    pub room_id: i64,
    pub parent_id: i64,
    pub room_name: String,
    pub preview: String,
    pub unread: i64,
}

/// LC-575: one recent-DM row on the Direct messages card.
pub struct DmRow {
    pub peer_id: String,
    pub label: String,
    /// First character seeds the initials avatar when `avatar_ext` is None.
    pub username: String,
    pub avatar_ext: Option<String>,
    pub unread: i64,
}

/// LC-575: one draft row on the Drafts card. `href` already resolves to the
/// right deep-link (`/dm/{peer}` for DM drafts, `/room/{id}` for channels).
pub struct DraftRow {
    pub href: String,
    pub label: String,
}
