#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::pinned::PinnedListRow;

/// LC-87: one row in the Files tab. Pre-resolved by the route so the
/// template stays presentation-only. `is_image` drives the thumbnail-
/// vs-icon branch. `kind_icon` is a one-letter glyph for non-image
/// rows ("V" video, "A" audio, "P" pdf, "F" other) to keep the bundle
/// small without shipping an icon set.
pub struct RoomFileRow {
    pub id: i64,
    pub message_id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub created_at: String,
    pub uploader_label: String,
    pub is_image: bool,
    pub kind_icon: &'static str,
}

/// LC-87: human-readable byte size for the Files tab. KiB / MiB
/// thresholds line up with upload-size logging elsewhere in the
/// codebase.
pub fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes}\u{00a0}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}\u{00a0}KiB", (bytes as f64) / 1024.0)
    } else {
        format!("{:.1}\u{00a0}MiB", (bytes as f64) / (1024.0 * 1024.0))
    }
}

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
    /// LC-87: file rows for the "Files" tab. Empty for other tabs.
    pub files: &'a [RoomFileRow],
    /// LC-87: active file-kind filter (`"all"` / `"image"` / `"video"`
    /// / `"audio"` / `"pdf"` / `"other"`). Drives the filter dropdown's
    /// selected option.
    pub files_kind: &'a str,
    /// LC-87: cursor for the "Load more" button, or `None` when the
    /// current page is the last one. Set by the route after a one-extra
    /// peek into the next page.
    pub files_next_cursor: Option<i64>,
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

/// LC-87: file-list fragment for HTMX pagination + filter swaps.
/// Returned by `GET /room/{id}/files` so the client can swap into
/// `#files-grid` without re-rendering the rest of the info page.
/// `append` toggles between "replace the grid" (filter change) and
/// "append rows" (load-more); the template branches on it to decide
/// whether to render the wrapper element.
#[derive(Template)]
#[template(path = "room/files_fragment.html")]
pub struct FileListFragment<'a> {
    pub room_id: i64,
    pub files: &'a [RoomFileRow],
    pub kind: &'a str,
    pub next_cursor: Option<i64>,
    pub append: bool,
}
