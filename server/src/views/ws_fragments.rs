use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom};
use crate::views::room::MessageView;
use crate::ws::events::ChatEvent;

#[derive(Template)]
#[template(path = "ws/new_message.html")]
pub struct NewMessageFragment<'a> {
    pub message: &'a MessageView,
}

#[derive(Template)]
#[template(path = "ws/edited_message.html")]
pub struct EditedMessageFragment<'a> {
    pub message: &'a MessageView,
}

#[derive(Template)]
#[template(path = "ws/deleted_message.html")]
pub struct DeletedMessageFragment {
    pub message_id: i64,
}

#[derive(Template)]
#[template(path = "ws/typing.html")]
pub struct TypingFragment<'a> {
    pub username: &'a str,
}

#[derive(Template)]
#[template(path = "ws/stopped_typing.html")]
pub struct StoppedTypingFragment;

#[derive(Template)]
#[template(path = "ws/reaction_update.html")]
pub struct ReactionUpdateFragment<'a> {
    pub message_id: i64,
    pub reactions: &'a [super::room::ReactionView],
}

/// Out-of-band swap that updates a sidebar unread badge. The badge id is
/// `unread-{kind}-{id}` where kind is "room" or "dm" and id is the room_id
/// (for rooms) or peer user_id (for DMs, from the badge owner's perspective).
/// `unread = 0` clears the badge; positive values render a count chip.
#[derive(Template)]
#[template(path = "ws/unread_badge.html")]
pub struct UnreadBadgeFragment<'a> {
    pub kind: &'a str,
    pub id: &'a str,
    pub unread: i64,
}

/// Out-of-band swap that updates the "Seen HH:MM" caption under one DM
/// message. `caption = None` clears the slot; `Some(text)` populates it. The
/// element id is `seen-{message_id}`, present in the DOM for every
/// own-authored DM message via `room/message.html`.
#[derive(Template)]
#[template(path = "ws/seen_indicator.html")]
pub struct SeenIndicatorFragment<'a> {
    pub message_id: i64,
    pub caption: Option<&'a str>,
}

/// OOB sidebar replacement. Used when the user's room/DM membership changes
/// so the new entry shows up live without a refresh. Renders the entire
/// sidebar partial wrapped to swap on the existing #sidebar element.
#[derive(Template)]
#[template(path = "ws/sidebar_update.html")]
pub struct SidebarUpdateFragment<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
}

/// Render a ChatEvent as an HTML fragment with hx-swap-oob attributes for
/// events that do not depend on the recipient. Per-recipient events
/// (NewMessage, MessageEdited, ReactionAdded/Removed, DmRead,
/// RoomMemberAdded/Removed) are rendered directly in the WS handler where
/// the recipient identity is available.
pub fn render_event(event: &ChatEvent) -> Option<String> {
    match event {
        ChatEvent::MessageDeleted { message_id, .. } => DeletedMessageFragment {
            message_id: *message_id,
        }
        .render()
        .ok(),
        ChatEvent::UserTyping { username, .. } => TypingFragment { username }.render().ok(),
        ChatEvent::UserStoppedTyping { .. } => StoppedTypingFragment.render().ok(),
        ChatEvent::NewMessage { .. }
        | ChatEvent::MessageEdited { .. }
        | ChatEvent::MessageRegrouped { .. }
        | ChatEvent::ReactionAdded { .. }
        | ChatEvent::ReactionRemoved { .. }
        | ChatEvent::RoomMemberAdded { .. }
        | ChatEvent::RoomMemberRemoved { .. }
        | ChatEvent::DmRead { .. }
        | ChatEvent::UserMuted { .. }
        | ChatEvent::UserBanned { .. }
        | ChatEvent::UserKicked { .. } => None,
    }
}
