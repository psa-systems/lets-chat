//! LC-72: versioned JSON API (`/api/v1`) authenticated by scoped bearer
//! tokens (`ApiAuth`). These routes are merged AFTER the cookie / 2FA /
//! maintenance middleware in `build_router`, so they bypass the browser
//! session entirely and authenticate only via `Authorization: Bearer`.
//!
//! Every route declares the scope it needs via `auth.require(...)`; a token
//! lacking the scope is refused 403 (a missing/expired/revoked token is 401,
//! handled by the extractor). Routes here are also still bounded by the
//! token owner's own room access - scopes narrow, never widen, the user.
//!
//! Scope reference lives in `docs/api.md`.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::collections::HashMap;

use crate::auth::ApiAuth;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

pub const SCOPE_MESSAGES_READ: &str = "messages:read";
pub const SCOPE_MESSAGES_WRITE: &str = "messages:write";
pub const SCOPE_ROOMS_READ: &str = "rooms:read";

/// Every scope a token may hold, for the management UI's checkboxes.
pub const ALL_SCOPES: &[&str] = &[SCOPE_MESSAGES_READ, SCOPE_MESSAGES_WRITE, SCOPE_ROOMS_READ];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me", get(get_me))
        .route("/api/v1/rooms", get(get_rooms))
        .route(
            "/api/v1/rooms/{room_id}/messages",
            get(get_messages).post(post_message),
        )
}

#[derive(Serialize)]
struct ApiMe {
    id: String,
    username: String,
    role: String,
}

/// GET /api/v1/me - the token owner's identity. Reachable by any valid
/// token (it reveals only the caller's own account).
async fn get_me(auth: ApiAuth) -> Json<ApiMe> {
    Json(ApiMe {
        id: auth.user.id.clone(),
        username: auth.user.username.clone(),
        role: auth.user.role.clone(),
    })
}

#[derive(Serialize)]
struct ApiRoom {
    id: i64,
    name: String,
    room_type: String,
}

/// GET /api/v1/rooms - non-DM rooms the token owner can see. Scope:
/// `rooms:read`.
async fn get_rooms(
    State(state): State<AppState>,
    auth: ApiAuth,
) -> Result<Json<Vec<ApiRoom>>, AppError> {
    auth.require(SCOPE_ROOMS_READ)?;
    let is_admin = auth.user.role == "admin";
    let rooms = db::chat::list_rooms(&state.chat, &auth.user.id, is_admin).await?;
    Ok(Json(
        rooms
            .into_iter()
            .map(|r| ApiRoom {
                id: r.id,
                name: r.name,
                room_type: r.room_type,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
struct ApiMessage {
    id: i64,
    room_id: i64,
    user_id: String,
    author: String,
    body: String,
    created_at: String,
}

async fn require_room_access(
    state: &AppState,
    auth: &ApiAuth,
    room_id: i64,
) -> Result<crate::models::Room, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = auth.user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &auth.user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(room)
}

/// GET /api/v1/rooms/{id}/messages - top-level messages in a room. Scope:
/// `messages:read`.
async fn get_messages(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path(room_id): Path<i64>,
) -> Result<Json<Vec<ApiMessage>>, AppError> {
    auth.require(SCOPE_MESSAGES_READ)?;
    require_room_access(&state, &auth, room_id).await?;
    let raw = db::chat::list_messages(&state.chat, room_id).await?;

    // Resolve author display labels once per distinct author.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut out = Vec::with_capacity(raw.len());
    for m in raw {
        let author = if let Some(n) = names.get(&m.user_id) {
            n.clone()
        } else {
            let label = db::auth::find_user_by_id(&state.auth, &m.user_id)
                .await?
                .map(|u| u.display_name.unwrap_or(u.username))
                .unwrap_or_else(|| "(unknown)".to_string());
            names.insert(m.user_id.clone(), label.clone());
            label
        };
        out.push(ApiMessage {
            id: m.id,
            room_id: m.room_id,
            user_id: m.user_id,
            author,
            body: m.body,
            created_at: m.created_at,
        });
    }
    Ok(Json(out))
}

#[derive(serde::Deserialize)]
struct PostMessageBody {
    body: String,
}

/// POST /api/v1/rooms/{id}/messages - post a message. Scope:
/// `messages:write`. Honors the same room access + ban/mute gates as the
/// web composer and broadcasts to connected clients.
async fn post_message(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path(room_id): Path<i64>,
    Json(payload): Json<PostMessageBody>,
) -> Result<Json<ApiMessage>, AppError> {
    auth.require(SCOPE_MESSAGES_WRITE)?;
    if auth.user.is_banned || auth.user.is_muted {
        return Err(AppError::Forbidden);
    }
    let body = payload.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("message body cannot be empty".into()));
    }
    let room = require_room_access(&state, &auth, room_id).await?;
    let new_id = db::chat::insert_message(&state.chat, room_id, &auth.user.id, body).await?;
    let message =
        super::room::finalize_message_send(&state, &room, &auth.user, new_id, body).await?;
    Ok(Json(ApiMessage {
        id: message.id,
        room_id: message.room_id,
        user_id: message.user_id,
        author: message.author_name,
        body: message.body,
        created_at: message.created_at,
    }))
}
