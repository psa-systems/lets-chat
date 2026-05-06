use askama::Template;

use crate::models::{Attachment, Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

pub struct MessageView {
    pub id: i64,
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
    /// File attachments linked to this message. Empty for plain text
    /// messages; the template only renders attachment markup when non-empty.
    pub attachments: Vec<Attachment>,
}

impl MessageView {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }

    /// True when the message has no body text and exactly one image
    /// attachment. The template renders this as an unbubbled image (LC-3).
    pub fn is_image_only(&self) -> bool {
        self.body.trim().is_empty() && self.attachments.len() == 1 && self.attachments[0].is_image()
    }

    /// HTML-escape the body, then wrap any detected URLs in `<a>` tags. The
    /// template renders the result with `|safe` so the anchors aren't
    /// double-escaped. Returning a String keeps the template free of
    /// linkify-aware loops while still producing inline anchors for LC-3
    /// link previews.
    pub fn body_html(&self) -> String {
        linkify_body(&self.body)
    }

    /// First URL in the body, or `None`. The template uses this to render
    /// at most one preview-card lazy-load shell per message so repeated
    /// links in a single body don't multiply network fetches.
    pub fn first_url(&self) -> Option<String> {
        let finder = linkify::LinkFinder::new();
        finder
            .links(&self.body)
            .find(|l| matches!(l.kind(), linkify::LinkKind::Url))
            .map(|l| l.as_str().to_string())
    }

    /// Percent-encoded form of `first_url()` for safe injection into the
    /// `hx-get` query string. Askama 0.12 has no urlencode built-in, so the
    /// view layer pre-encodes.
    pub fn first_url_encoded(&self) -> Option<String> {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        self.first_url()
            .map(|u| utf8_percent_encode(&u, NON_ALPHANUMERIC).to_string())
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn linkify_body(body: &str) -> String {
    let finder = linkify::LinkFinder::new();
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    for link in finder.links(body) {
        if !matches!(link.kind(), linkify::LinkKind::Url) {
            continue;
        }
        let start = link.start();
        let end = link.end();
        if start > cursor {
            out.push_str(&html_escape(&body[cursor..start]));
        }
        let url = link.as_str();
        out.push_str("<a href=\"");
        out.push_str(&html_escape(url));
        out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\" class=\"text-blue-600 hover:underline\">");
        out.push_str(&html_escape(url));
        out.push_str("</a>");
        cursor = end;
    }
    if cursor < body.len() {
        out.push_str(&html_escape(&body[cursor..]));
    }
    out
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
