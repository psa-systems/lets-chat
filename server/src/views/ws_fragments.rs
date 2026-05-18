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
    /// "none" | "except_mentions" | "all". The WS badge is rendered for
    /// background recipients in the chat path, where the mute filter in
    /// `render_new_message_or_bump` has already short-circuited muted rooms.
    /// Pass `"none"` here so the template logic stays unified with the
    /// page-render include site.
    pub mute_mode: &'a str,
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
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
}

/// OOB-append a single thread reply into `#thread-replies-{parent_id}`. The
/// container only exists in the DOM when the recipient has the thread panel
/// open for that parent, so HTMX silently drops the swap otherwise.
#[derive(Template)]
#[template(path = "ws/thread_reply.html")]
pub struct ThreadReplyOobFragment<'a> {
    pub parent_id: i64,
    pub message: &'a super::room::MessageView,
}

#[derive(Template)]
#[template(path = "ws/thread_typing.html")]
pub struct ThreadTypingFragment<'a> {
    pub parent_id: i64,
    pub username: &'a str,
}

#[derive(Template)]
#[template(path = "ws/thread_stopped_typing.html")]
pub struct ThreadStoppedTypingFragment {
    pub parent_id: i64,
}

/// OOB-appended notification trigger. The client's `#lc-notify-bus`
/// MutationObserver consumes this element to fire a title flash, favicon
/// dot, optional sound, and (when the tab is hidden) a browser
/// Notification.
#[derive(Template)]
#[template(path = "ws/mentioned.html")]
pub struct MentionedFragment<'a> {
    pub kind: &'a str,
    pub room_id: i64,
    pub room_type: &'a str,
    pub room_label: &'a str,
    pub message_id: i64,
    pub author_label: &'a str,
    pub snippet: &'a str,
    pub target_path: &'a str,
}

/// OOB-appended decrement signal: an earlier mention was retracted (the
/// message was edited to remove the @-token, or deleted). The client
/// decrements its in-memory unread-mention count for the room.
#[derive(Template)]
#[template(path = "ws/mention_cleared.html")]
pub struct MentionClearedFragment {
    pub room_id: i64,
    pub message_id: i64,
}

/// Tiny inline script that updates every avatar status dot for a user across
/// the rendered DOM (sidebar entries, message rows, DM headers). Uses a
/// CSS-attribute selector instead of OOB-by-id because the same user's dot
/// appears many times per page and HTMX OOB swaps only the first id match.
/// Fields are pre-encoded as JSON literals to defend against quote/script
/// injection in the user-controlled custom status text.
#[derive(Template)]
#[template(path = "ws/user_status_update.html")]
pub struct UserStatusUpdateFragment {
    pub user_id_json: String,
    pub status_json: String,
    pub custom_status_json: String,
}

/// OOB-appended WebRTC call signal. The client's `#lc-call-bus`
/// MutationObserver consumes this element and feeds the `kind` + `payload`
/// into the per-DM `RTCPeerConnection` state machine. `payload` carries an
/// opaque SDP/ICE blob for media kinds and is absent for the pure-control
/// kinds (invite/accept/reject/cancel/hangup).
#[derive(Template)]
#[template(path = "ws/call_signal.html")]
pub struct CallSignalFragment<'a> {
    pub room_id: i64,
    pub from_user_id: &'a str,
    pub from_name: &'a str,
    pub kind: &'a str,
    pub payload: Option<&'a str>,
}

/// OOB-appended voice-channel event. The client's `#lc-voice-bus`
/// MutationObserver consumes this and feeds it into the per-channel mesh
/// of `RTCPeerConnection`s. `kind` is `joined` / `left` / `roster` /
/// `offer` / `answer` / `ice`; the other fields carry whichever payload
/// that kind needs (empty string when not applicable).
#[derive(Template)]
#[template(path = "ws/voice_event.html")]
pub struct VoiceEventFragment<'a> {
    pub room_id: i64,
    pub kind: &'a str,
    pub user_id: &'a str,
    pub username: &'a str,
    pub peers_json: &'a str,
    pub payload: Option<&'a str>,
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
        ChatEvent::ThreadTyping {
            parent_id,
            username,
            ..
        } => ThreadTypingFragment {
            parent_id: *parent_id,
            username,
        }
        .render()
        .ok(),
        ChatEvent::ThreadStoppedTyping { parent_id, .. } => ThreadStoppedTypingFragment {
            parent_id: *parent_id,
        }
        .render()
        .ok(),
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
        | ChatEvent::UserKicked { .. }
        | ChatEvent::ThreadReply { .. }
        | ChatEvent::EnclaveMemberAdded { .. }
        | ChatEvent::EnclaveMemberRemoved { .. }
        | ChatEvent::EnclaveRoomAdded { .. }
        | ChatEvent::EnclaveRoomRemoved { .. }
        | ChatEvent::SidebarCategoriesChanged { .. }
        | ChatEvent::EnclaveInvitationCreated { .. }
        | ChatEvent::EnclaveInvitationResolved { .. }
        | ChatEvent::Mentioned { .. }
        | ChatEvent::MentionCleared { .. }
        | ChatEvent::RoomNotifyPrefsChanged { .. }
        | ChatEvent::DmMuteChanged { .. }
        | ChatEvent::MessagePinned { .. }
        | ChatEvent::MessageUnpinned { .. }
        | ChatEvent::CallSignal { .. }
        | ChatEvent::VoiceJoined { .. }
        | ChatEvent::VoiceLeft { .. }
        | ChatEvent::VoiceRoster { .. }
        | ChatEvent::VoiceSignal { .. } => None,
        ChatEvent::UserStatusChanged {
            user_id,
            status,
            custom_status,
        } => UserStatusUpdateFragment {
            user_id_json: serde_json::to_string(user_id).ok()?,
            status_json: serde_json::to_string(status).ok()?,
            custom_status_json: serde_json::to_string(custom_status).ok()?,
        }
        .render()
        .ok(),
    }
}
