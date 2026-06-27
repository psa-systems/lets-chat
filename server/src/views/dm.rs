#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::room::MessageView;

#[derive(Template)]
#[template(path = "dm/page.html")]
pub struct DmPage<'a> {
    pub user: &'a User,
    pub peer: &'a User,
    pub room: &'a Room,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
    /// "none" | "all". Drives the `Mute` checkbox `checked` state in the
    /// DM header partial.
    pub mute_mode: String,
    /// Pre-rendered pinned-strip HTML (or empty string when there are
    /// zero pins). Same shape as `RoomPage::pinned_strip_html`.
    pub pinned_strip_html: String,
    /// LC-64: the user's persisted draft body for this DM room, if
    /// any. Same shape as `RoomPage::initial_draft`. Empty string =
    /// no draft (or stale + just purged).
    pub initial_draft: String,
    /// LC-500: configured max upload size (bytes), surfaced to the composer for
    /// client-side validation. Same as `RoomPage::max_upload_bytes`.
    pub max_upload_bytes: i64,
    /// LC-486: an operator LLM is configured. Drives `data-lc-llm` on the page
    /// root, which CSS-gates the per-message Translate action.
    pub llm_available: bool,
}

impl DmPage<'_> {
    /// LC-332: the message-length cap, surfaced to `room/composer.html` (included
    /// here) as `data-lc-maxchars` so the character counter reads a single source
    /// of truth instead of a magic number baked into the template.
    pub fn max_message_chars(&self) -> usize {
        crate::routes::room::MAX_MESSAGE_CHARS
    }
}
