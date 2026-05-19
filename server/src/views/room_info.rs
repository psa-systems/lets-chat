use askama::Template;

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::pinned::PinnedListRow;

/// LC-86: room info page (`/room/{id}/info`). Renders the topic +
/// description + wiki ("Docs" tab) or the pinned-message list ("Pinned"
/// tab). The tab nav lives at the top; switching is a normal navigation
/// (`?tab=`) so the page is fully accessible without HTMX.
#[derive(Template)]
#[template(path = "room/info.html")]
pub struct RoomInfoPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    /// `"docs"` (default) or `"pinned"`. Drives the tab nav's selected
    /// state and the content branch.
    pub active_tab: &'a str,
    /// Pre-rendered wiki HTML (markdown -> HTML via `views::markdown`).
    /// Empty string when no wiki has been written yet.
    pub wiki_html: String,
    /// Pre-rendered description HTML (same pipeline). Empty string when
    /// no description has been set.
    pub description_html: String,
    /// Display label for `wiki_updated_by`, resolved server-side.
    pub wiki_updated_by_label: Option<String>,
    /// True when the viewer's effective role in this room is moderator+
    /// (LC-84). Drives the "Edit wiki" button.
    pub can_edit_wiki: bool,
    /// True when the viewer is allowed to grant/revoke per-room role
    /// overrides (LC-84). Drives the description edit affordance.
    pub can_manage_overrides: bool,
    /// Pinned messages for the "Pinned" tab. Empty for the docs tab.
    pub pinned: &'a [PinnedListRow],
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

/// Inline edit form for the wiki, swapped into `#wiki-content` via
/// `hx-get="/room/{id}/wiki/edit"`. Mod+ only.
#[derive(Template)]
#[template(path = "room/wiki_edit.html")]
pub struct WikiEditFragment<'a> {
    pub room_id: i64,
    pub body: &'a str,
}

/// Inline edit form for the description. Admin-tier only (same auth as
/// the moderators page). Mirrors `WikiEditFragment` so the two sections
/// look and behave the same way.
#[derive(Template)]
#[template(path = "room/description_edit.html")]
pub struct DescriptionEditFragment<'a> {
    pub room_id: i64,
    pub body: &'a str,
}

/// Rendered description fragment, returned by `GET /room/{id}/description`
/// (cancel out of edit) and `PATCH /room/{id}/description` (after save).
#[derive(Template)]
#[template(path = "room/description_view.html")]
pub struct DescriptionViewFragment {
    pub room_id: i64,
    pub html: String,
    pub can_edit_description: bool,
}

/// Rendered wiki body (after a successful PATCH). Same shell the page
/// uses for the view-mode wiki section so the swap is in-place.
#[derive(Template)]
#[template(path = "room/wiki_view.html")]
pub struct WikiViewFragment<'a> {
    pub room_id: i64,
    pub html: String,
    pub updated_at: Option<&'a str>,
    pub updated_by_label: Option<&'a str>,
    pub can_edit_wiki: bool,
}
