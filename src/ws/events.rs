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
}

/// Control frames sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientControl {
    Subscribe { room_id: i64 },
    Unsubscribe { room_id: i64 },
    Typing { room_id: i64 },
}
