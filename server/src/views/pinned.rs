use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One pin row in the header strip. The route handler pre-truncates the
/// body to `snippet`, pre-resolves `author_label` (display_name or
/// username), and pre-formats `pinned_at` so the template stays simple.
pub struct PinnedStripRow {
    pub message_id: i64,
    pub author_label: String,
    pub snippet: String,
    pub pinned_at: String,
}

/// One pin row in the full pin-list page. Carries author + pinner labels
/// resolved from a single bulk auth lookup, plus the full body (the
/// list page does not truncate).
pub struct PinnedListRow {
    pub message_id: i64,
    pub author_label: String,
    pub pinner_label: String,
    pub pinned_at: String,
    pub body: String,
}

#[derive(Template)]
#[template(path = "partials/pinned_strip.html")]
pub struct PinnedStripFragment<'a> {
    pub room_id: i64,
    pub total_count: i64,
    /// `total_count > top_pins.len()` precomputed so the template can
    /// avoid an `as i64` cast (Askama 0.12 chokes on it in conditions).
    pub has_more: bool,
    pub pin_path: &'a str,
    pub top_pins: Vec<PinnedStripRow>,
    /// True when this fragment is being broadcast as an OOB swap (WS
    /// event or HTTP response that also carries an unrelated primary
    /// target). False on the initial page render.
    pub oob: bool,
}

#[derive(Template)]
#[template(path = "room/pins.html")]
pub struct PinnedListPage<'a> {
    pub user: &'a User,
    pub asset_version: &'a str,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub room_label: String,
    pub back_path: String,
    pub pins: Vec<PinnedListRow>,
}
