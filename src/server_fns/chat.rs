use dioxus::prelude::*;

use crate::models::{Message, Room};

#[server]
pub async fn list_rooms() -> Result<Vec<Room>, ServerFnError> {
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::list_rooms(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn get_room(room_id: i64) -> Result<Option<Room>, ServerFnError> {
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::get_room(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn list_messages(room_id: i64) -> Result<Vec<Message>, ServerFnError> {
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::list_messages(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn send_message(
    room_id: i64,
    author: String,
    body: String,
) -> Result<i64, ServerFnError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(ServerFnError::new("message body cannot be empty"));
    }
    let author = author.trim();
    if author.is_empty() {
        return Err(ServerFnError::new("author cannot be empty"));
    }
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::insert_message(pool, room_id, author, body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
