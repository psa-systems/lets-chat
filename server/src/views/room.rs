use askama::Template;

use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

pub struct MessageView {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub body: String,
    pub reactions: Vec<ReactionView>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub viewer_id: String,
    /// HH:MM peer-read timestamp shown under this message in a DM, or None.
    /// Only one own-authored message in a DM should have this set at a time.
    pub seen_caption: Option<String>,
    /// True when this message follows another message from the same author
    /// within MESSAGE_GROUPING_WINDOW. Hides the username/timestamp header.
    pub is_follow_up: bool,
    /// True for the first message in the page that the viewer has not yet
    /// read on this server. Renders an "Unread messages" divider above the
    /// message and tells the auto-scroll script to anchor here on load.
    pub show_unread_divider: bool,
    /// Count of non-deleted thread replies rooted at this message. Zero
    /// suppresses the "N replies" pill. Only meaningful for top-level
    /// messages; replies always render with `reply_count = 0`.
    pub reply_count: i64,
    /// `Some(N)` when this view represents a thread reply (rendered inside
    /// the panel). `None` for top-level messages in the main feed.
    pub parent_id: Option<i64>,
}

impl MessageView {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
}

pub struct ReactionView {
    pub emoji: String,
    pub count: i64,
    pub viewer_reacted: bool,
}

#[derive(Template)]
#[template(path = "room/page.html")]
pub struct RoomPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "room/composer.html")]
pub struct ComposerFragment<'a> {
    pub room: &'a Room,
}

#[derive(Template)]
#[template(path = "room/edit_form.html")]
pub struct EditFormFragment<'a> {
    pub message_id: i64,
    pub current_body: &'a str,
}

#[derive(Template)]
#[template(path = "room/message.html")]
pub struct SingleMessageFragment<'a> {
    pub message: &'a MessageView,
    pub oob: bool,
}

#[derive(Template)]
#[template(path = "partials/reaction_bar.html")]
pub struct ReactionBarFragment<'a> {
    pub message_id: i64,
    pub reactions: &'a [ReactionView],
}

/// Right-side thread drawer. Replaces `#thread-panel` outerHTML when opened.
#[derive(Template)]
#[template(path = "room/thread_panel.html")]
pub struct ThreadPanelFragment<'a> {
    pub room: &'a Room,
    pub parent: &'a MessageView,
    pub replies: &'a [MessageView],
}

/// Empty thread panel container, used to close the drawer.
#[derive(Template)]
#[template(path = "room/thread_panel_closed.html")]
pub struct ThreadPanelClosedFragment;

/// Single reply rendered inside the panel's reply list.
#[derive(Template)]
#[template(path = "room/thread_reply.html")]
pub struct ThreadReplyFragment<'a> {
    pub message: &'a MessageView,
}

/// "N replies" pill rendered under a top-level message. Standalone fragment
/// so the WS layer can OOB-swap it when reply_count changes.
#[derive(Template)]
#[template(path = "room/reply_count.html")]
pub struct ReplyCountFragment {
    pub message_id: i64,
    pub room_id: i64,
    pub reply_count: i64,
    pub oob: bool,
}
