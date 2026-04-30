use askama::Template;

use crate::ws::events::ChatEvent;

#[derive(Template)]
#[template(path = "ws/new_message.html")]
pub struct NewMessageFragment<'a> {
    pub message_id: i64,
    pub room_id: i64,
    pub username: &'a str,
    pub created_at: &'a str,
    pub body: &'a str,
}

#[derive(Template)]
#[template(path = "ws/edited_message.html")]
pub struct EditedMessageFragment<'a> {
    pub message_id: i64,
    pub new_body: &'a str,
    pub edited_at: &'a str,
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

/// Render a ChatEvent as an HTML fragment with hx-swap-oob attributes.
/// Returns None for events that don't produce a fragment for the given user
/// (e.g., a global UserBanned event for the current user - the page should
/// redirect, not swap).
pub fn render_event(event: &ChatEvent) -> Option<String> {
    match event {
        ChatEvent::NewMessage { message, .. } => NewMessageFragment {
            message_id: message.id,
            room_id: message.room_id,
            username: &message.author_name,
            created_at: &message.created_at,
            body: &message.body,
        }
        .render()
        .ok(),
        ChatEvent::MessageEdited {
            message_id,
            new_body,
            edited_at,
            ..
        } => EditedMessageFragment {
            message_id: *message_id,
            new_body,
            edited_at,
        }
        .render()
        .ok(),
        ChatEvent::MessageDeleted { message_id, .. } => DeletedMessageFragment {
            message_id: *message_id,
        }
        .render()
        .ok(),
        ChatEvent::UserTyping { username, .. } => TypingFragment { username }.render().ok(),
        ChatEvent::UserStoppedTyping { .. } => StoppedTypingFragment.render().ok(),
        ChatEvent::ReactionAdded { .. } | ChatEvent::ReactionRemoved { .. } => None,
        ChatEvent::RoomMemberAdded { .. }
        | ChatEvent::RoomMemberRemoved { .. }
        | ChatEvent::DmRead { .. }
        | ChatEvent::UserMuted { .. }
        | ChatEvent::UserBanned { .. }
        | ChatEvent::UserKicked { .. } => None,
    }
}
