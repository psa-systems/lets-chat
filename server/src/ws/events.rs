use serde::{Deserialize, Serialize};

use crate::models::Message;

/// Events sent from server to client over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    NewMessage {
        message: Message,
        is_dm: bool,
    },
    MessageEdited {
        message_id: i64,
        room_id: i64,
        new_body: String,
        edited_at: String,
    },
    /// Emitted by the delete handler when removing a header message exposes a
    /// follow-up that should be promoted to a header. Recipients re-render the
    /// referenced message with the current grouping flag.
    MessageRegrouped {
        message_id: i64,
        room_id: i64,
    },
    MessageDeleted {
        message_id: i64,
        room_id: i64,
    },
    UserTyping {
        room_id: i64,
        user_id: String,
        username: String,
    },
    UserStoppedTyping {
        room_id: i64,
        user_id: String,
    },
    RoomMemberAdded {
        room_id: i64,
        user_id: String,
    },
    DmRead {
        room_id: i64,
        user_id: String,
        last_read_message_id: i64,
        read_at: String,
    },
    RoomMemberRemoved {
        room_id: i64,
        user_id: String,
    },
    UserMuted {
        user_id: String,
        muted_until: Option<String>,
    },
    UserBanned {
        user_id: String,
    },
    UserKicked {
        user_id: String,
        room_id: i64,
    },
    ReactionAdded {
        message_id: i64,
        room_id: i64,
        emoji: String,
        user_id: String,
    },
    ReactionRemoved {
        message_id: i64,
        room_id: i64,
        emoji: String,
        user_id: String,
    },
    EnclaveMemberAdded {
        enclave_id: i64,
        user_id: String,
    },
    EnclaveMemberRemoved {
        enclave_id: i64,
        user_id: String,
    },
    EnclaveRoomAdded {
        enclave_id: i64,
        room_id: i64,
    },
    EnclaveRoomRemoved {
        enclave_id: i64,
        room_id: i64,
    },
    EnclaveInvitationCreated {
        invitee_id: String,
    },
    EnclaveInvitationResolved {
        invitee_id: String,
    },
    UserStatusChanged {
        user_id: String,
        status: String,
        custom_status: Option<String>,
    },
    /// New thread reply rooted at `parent_id`. Recipients with the panel open
    /// for that parent append the reply; everyone updates the parent's
    /// "N replies" pill.
    ThreadReply {
        parent_id: i64,
        message: Message,
    },
    /// Typing indicator scoped to the thread panel.
    ThreadTyping {
        room_id: i64,
        parent_id: i64,
        user_id: String,
        username: String,
    },
    ThreadStoppedTyping {
        room_id: i64,
        parent_id: i64,
        user_id: String,
    },
    /// A user was @-mentioned in a room message, or a DM was sent to them.
    /// Routed via `Hub::broadcast_to_user(mentioned_user_id, ...)`.
    Mentioned {
        /// "mention" for a real `@username` ping in a room; "dm" for an
        /// implicit DM ping. The client uses this to label the notification.
        kind: String,
        room_id: i64,
        /// "public" | "private" | "dm" - lets the client format the
        /// notification title without an extra DB lookup.
        room_type: String,
        /// Display label for the room (e.g. "#general") or DM peer name.
        room_label: String,
        message_id: i64,
        mentioned_user_id: String,
        author_label: String,
        /// First ~140 chars of the body, plain-text. Mention chips and links
        /// are stripped to keep the notification readable.
        snippet: String,
        /// "/room/{id}" or "/dm/{peer_id}" - target path for the click handler.
        target_path: String,
    },
    /// A previously-fired mention is no longer current (the message was
    /// edited to remove the @-token, or the message was deleted). The client
    /// decrements its in-memory unread-mention count for that room.
    MentionCleared {
        room_id: i64,
        mentioned_user_id: String,
        message_id: i64,
    },
    /// Per-user notification of a notify-prefs change. Recipients re-render
    /// their sidebar OOB so the muted-room class flips and badges hide/show
    /// across all of their open tabs. Routed via
    /// `Hub::broadcast_to_user(user_id, ...)`.
    RoomNotifyPrefsChanged {
        user_id: String,
        room_id: i64,
        mute_mode: String,
    },
}

/// Control frames sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientControl {
    Subscribe { room_id: i64 },
    Unsubscribe { room_id: i64 },
    Typing { room_id: i64 },
    ThreadTyping { room_id: i64, parent_id: i64 },
}
