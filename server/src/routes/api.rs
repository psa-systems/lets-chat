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

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::auth::ApiAuth;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

pub const SCOPE_MESSAGES_READ: &str = "messages:read";
pub const SCOPE_MESSAGES_WRITE: &str = "messages:write";
pub const SCOPE_ROOMS_READ: &str = "rooms:read";
/// LC-78: scope for `POST /api/v1/bridges/{id}/messages`. Strictly more
/// powerful than `messages:write` because it lets the caller post under an
/// arbitrary foreign display name; a `messages:write`-only token cannot
/// reach the bridge endpoint, and the bridge endpoint rejects calls that
/// hold `messages:write` but lack this scope.
pub const SCOPE_BRIDGE_POST: &str = "bridge:post";
/// LC-78: scope for `POST /api/v1/bridges/{id}/heartbeat`. Separated from
/// `bridge:post` so a daemon's read/heartbeat token can be split off from
/// its write token if an operator wants narrower scopes.
pub const SCOPE_BRIDGE_HEARTBEAT: &str = "bridge:heartbeat";

/// Every scope a token may hold, for the management UI's checkboxes.
pub const ALL_SCOPES: &[&str] = &[
    SCOPE_MESSAGES_READ,
    SCOPE_MESSAGES_WRITE,
    SCOPE_ROOMS_READ,
    SCOPE_BRIDGE_POST,
    SCOPE_BRIDGE_HEARTBEAT,
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me", get(get_me))
        .route("/api/v1/rooms", get(get_rooms))
        .route(
            "/api/v1/rooms/{room_id}/messages",
            get(get_messages).post(post_message),
        )
        .route(
            "/api/v1/bridges/{bridge_id}/messages",
            axum::routing::post(post_bridge_message),
        )
        .route(
            "/api/v1/bridges/{bridge_id}/heartbeat",
            axum::routing::post(post_bridge_heartbeat),
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
    auth.require_not_bridge()?;
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

/// LC-78: default page size for the paginated read. Matches the size most
/// daemons (Matrix appservice initial-sync) request without an explicit
/// limit; tuned to one screen of history.
const PAGINATED_DEFAULT_LIMIT: i64 = 50;
/// LC-78: hard cap on a single page so a caller cannot turn the bounded
/// read back into the previous "fetch every message in the room" shape.
const PAGINATED_MAX_LIMIT: i64 = 200;

#[derive(Deserialize)]
struct GetMessagesQuery {
    /// Cursor for the next page: results are strictly older than this id.
    /// Omit to start at the most recent message.
    #[serde(default)]
    before_id: Option<i64>,
    /// Page size. Clamped to `[1, PAGINATED_MAX_LIMIT]`; default
    /// `PAGINATED_DEFAULT_LIMIT`.
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Serialize)]
struct ApiMessagePage {
    messages: Vec<ApiMessage>,
    /// Cursor to feed back as `before_id` on the next request. `None` when
    /// the page returned fewer than `limit` rows (history exhausted) or
    /// the room is empty.
    next_cursor: Option<i64>,
}

/// GET /api/v1/rooms/{id}/messages - top-level messages in a room. Scope:
/// `messages:read`.
///
/// LC-78: paginated. Returns up to `limit` rows in `id DESC` order with a
/// `next_cursor` to walk older history. The bounded shape was added so the
/// LC-78 Matrix-bridge daemon's initial-sync has a deterministic paging
/// contract; this endpoint is the only API-side reader, so the change is
/// breaking only for API callers (the web/HTMX room render still uses the
/// unbounded `list_messages` directly, untouched).
async fn get_messages(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path(room_id): Path<i64>,
    Query(q): Query<GetMessagesQuery>,
) -> Result<Json<ApiMessagePage>, AppError> {
    auth.require_not_bridge()?;
    auth.require(SCOPE_MESSAGES_READ)?;
    require_room_access(&state, &auth, room_id).await?;
    let limit = q
        .limit
        .unwrap_or(PAGINATED_DEFAULT_LIMIT)
        .clamp(1, PAGINATED_MAX_LIMIT);
    let raw = db::chat::list_messages_paginated(&state.chat, room_id, q.before_id, limit).await?;

    // `next_cursor` is None when fewer than `limit` rows returned (exhaust)
    // or the result set is empty. Otherwise it is the smallest id returned,
    // which the caller feeds back as `before_id` on the next request.
    let next_cursor = if (raw.len() as i64) < limit {
        None
    } else {
        raw.last().map(|m| m.id)
    };

    // Resolve author display labels once per distinct author.
    let mut names: HashMap<String, String> = HashMap::new();
    let mut messages = Vec::with_capacity(raw.len());
    for m in raw {
        // Bridge messages snapshot their own author label on the row; we
        // surface that as the JSON `author` so a daemon's read-side matches
        // its post-side without joining to the bridges table.
        let author = if let Some(name) = m.bridge_foreign_name.clone() {
            name
        } else if let Some(n) = names.get(&m.user_id) {
            n.clone()
        } else {
            let label = db::auth::find_user_by_id(&state.auth, &m.user_id)
                .await?
                .map(|u| u.display_name.unwrap_or(u.username))
                .unwrap_or_else(|| "(unknown)".to_string());
            names.insert(m.user_id.clone(), label.clone());
            label
        };
        messages.push(ApiMessage {
            id: m.id,
            room_id: m.room_id,
            user_id: m.user_id,
            author,
            body: m.body,
            created_at: m.created_at,
        });
    }
    Ok(Json(ApiMessagePage {
        messages,
        next_cursor,
    }))
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
    auth.require_not_bridge()?;
    auth.require(SCOPE_MESSAGES_WRITE)?;
    if auth.user.is_banned || auth.user.is_muted {
        return Err(AppError::Forbidden);
    }
    let body = payload.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("message body cannot be empty".into()));
    }
    // LC-153: same length cap as the web composer; the bearer-token API is
    // otherwise an unbounded-body amplification path.
    super::room::check_message_length(body)?;
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

/// LC-78: per-bridge cap on the `bridge-messages` endpoint. Higher than the
/// human MESSAGE_RATE_PER_MIN because federated Matrix rooms can be bursty,
/// but still bounds what one compromised daemon can flood into a room.
const BRIDGE_MESSAGE_RATE_PER_MIN: u32 = 600;

#[derive(serde::Deserialize)]
struct PostBridgeMessageBody {
    body: String,
    foreign_name: String,
    /// LC-78-AVATAR-PROXY (v2): an absolute http(s) URL the server fetches
    /// once and caches; viewers' browsers only ever hit the same-origin
    /// proxy URL. Validated at submit time (URL shape + SSRF re-resolve),
    /// rejected with 400 on failure. Set
    /// `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` to restore v1's
    /// reject-non-null behavior (this field then must be absent or null).
    #[serde(default)]
    foreign_avatar: Option<String>,
}

/// LC-78: POST /api/v1/bridges/{bridge_id}/messages - post a foreign-protocol
/// message as a per-message synthetic actor. Scope: `bridge:post` (strictly
/// more powerful than `messages:write` because the caller chooses the
/// rendered display name; isolated to its own endpoint so a
/// `messages:write`-only token cannot reach the override).
///
/// The bridge daemon (LC-72 / LC-73) authenticates as the bot user that
/// owns the bridge row; the room is derived from the bridge, the kind is
/// snapshotted from the bridge row, and the foreign name + body are
/// snapshotted onto the message row so the render survives bridge-row
/// removal under stop-new lifecycle.
async fn post_bridge_message(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path(bridge_id): Path<i64>,
    Json(payload): Json<PostBridgeMessageBody>,
) -> Result<Json<ApiMessage>, AppError> {
    auth.require(SCOPE_BRIDGE_POST)?;
    if auth.user.is_banned || auth.user.is_muted {
        return Err(AppError::Forbidden);
    }
    // LC-78-AVATAR-PROXY: validate + hash the foreign avatar URL up-front.
    // SSRF re-resolve happens here (submit time) AND in the fetch task
    // (fetch time per LC-152's pattern). Failure is fail-loud 400, mirroring
    // the v1 reject posture; operators flipping the gate off get the v1
    // policy back without changing the endpoint shape.
    let foreign_avatar_hash: Option<String> = match payload.foreign_avatar.as_deref() {
        None => None,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => {
            if !crate::bridge_avatar::proxy_enabled() {
                return Err(AppError::BadRequest(
                    "foreign_avatar rejected by operator (LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false)".into(),
                ));
            }
            let trimmed = s.trim();
            if trimmed.len() > 2048 {
                return Err(AppError::BadRequest("foreign_avatar URL too long".into()));
            }
            let parsed = url::Url::parse(trimmed).map_err(|_| {
                AppError::BadRequest("foreign_avatar must be a valid URL".into())
            })?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(AppError::BadRequest(
                    "foreign_avatar must be an http(s) URL".into(),
                ));
            }
            if !crate::ssrf::host_resolves_public(&parsed).await {
                return Err(AppError::BadRequest(
                    "foreign_avatar host does not resolve public".into(),
                ));
            }
            let hash = crate::bridge_avatar::canonical_hash(trimmed)
                .ok_or_else(|| AppError::BadRequest("foreign_avatar URL invalid".into()))?;
            let inserted = db::bridge_avatar_proxies::upsert_pending(
                &state.chat,
                &hash,
                parsed.as_str(),
            )
            .await?;
            if inserted {
                // Fire-and-forget. Errors land in the row's `failure_reason`;
                // the render falls back to initials via <img onerror>.
                let chat = state.chat.clone();
                let url = parsed.to_string();
                let hash_owned = hash.clone();
                tokio::spawn(async move {
                    let client = match reqwest::Client::builder()
                        .user_agent("lets-chat-bridge-avatar/1")
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                    {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!(error = %e, "bridge-avatar reqwest client build failed");
                            let _ = db::bridge_avatar_proxies::mark_failed(
                                &chat,
                                &hash_owned,
                                "client build failed",
                            )
                            .await;
                            return;
                        }
                    };
                    crate::bridge_avatar::fetch_and_cache(&chat, &client, &hash_owned, &url).await;
                });
            }
            Some(hash)
        }
    };
    let bridge = db::bridges::find_by_id(&state.chat, bridge_id)
        .await?
        .ok_or(AppError::NotFound)?;
    // Ownership: the authenticated bot must be the bridge's registered bot.
    // A token from a different bot cannot post to this bridge even if it
    // somehow holds `bridge:post`.
    if bridge.bot_user_id != auth.user.id {
        return Err(AppError::Forbidden);
    }
    let body = payload.body.trim();
    let foreign_name = payload.foreign_name.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("message body cannot be empty".into()));
    }
    if foreign_name.is_empty() {
        return Err(AppError::BadRequest("foreign_name cannot be empty".into()));
    }
    super::room::check_message_length(body)?;
    // Bound foreign_name to a reasonable display-label length. The render
    // path treats it as text-only (escaped), but a multi-kilobyte name still
    // bloats the message row and the LC-75 outgoing-webhook payload.
    if foreign_name.len() > 256 {
        return Err(AppError::BadRequest("foreign_name too long".into()));
    }
    // Per-bridge rate limit, keyed by bridge id (NOT bot user id) so two
    // bridges owned by the same bot have independent buckets.
    if let crate::rate_limit::Outcome::Deny { retry_after } = state.rate_limits.check(
        crate::rate_limit::RateLimitKind::BridgeMessage,
        &bridge_id.to_string(),
        BRIDGE_MESSAGE_RATE_PER_MIN,
    ) {
        return Err(AppError::TooManyRequests(
            "bridge message rate limit exceeded".into(),
            retry_after,
        ));
    }
    let room = db::chat::get_room(&state.chat, bridge.room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let new_id = db::chat::insert_bridge_message(
        &state.chat,
        bridge.room_id,
        bridge.id,
        foreign_name,
        foreign_avatar_hash.as_deref(),
        &bridge.kind,
        body,
    )
    .await?;
    super::room::finalize_bridge_message_send(
        &state,
        &room,
        bridge.id,
        foreign_name,
        &bridge.kind,
        new_id,
        body,
    )
    .await?;
    let raw = db::chat::get_message(&state.chat, new_id)
        .await?
        .ok_or(AppError::Internal(
            "freshly inserted bridge message vanished".into(),
        ))?;
    Ok(Json(ApiMessage {
        id: raw.id,
        room_id: raw.room_id,
        user_id: String::new(),
        author: foreign_name.to_string(),
        body: raw.body,
        created_at: raw.created_at,
    }))
}

#[derive(Deserialize, Default)]
struct HeartbeatBody {
    /// Optional daemon-reported error string. `None` = healthy; `Some` puts
    /// the bridge in `errored` status and stores the message for the admin
    /// UI. Cap the length so a daemon cannot bloat the row.
    #[serde(default)]
    error: Option<String>,
}

#[derive(Serialize)]
struct HeartbeatAck {
    ok: bool,
    status: &'static str,
}

/// Maximum length of a daemon-reported error string. Bounds row growth + UI
/// rendering; longer messages are truncated rather than rejected (the
/// daemon should not refuse to heartbeat just because its error message
/// is verbose).
const HEARTBEAT_ERROR_MAX_BYTES: usize = 4096;

/// LC-78: POST /api/v1/bridges/{bridge_id}/heartbeat - record a daemon
/// liveness ping. Scope: `bridge:heartbeat`. The bridge daemon SHOULD POST
/// this on its own interval (typical: 30s); the admin UI surfaces `stale`
/// when the most-recent heartbeat is older than 3x interval (chunk 7).
/// Owner-gated: the authenticated bot must be the bridge's registered bot.
async fn post_bridge_heartbeat(
    State(state): State<AppState>,
    auth: ApiAuth,
    Path(bridge_id): Path<i64>,
    payload: Option<Json<HeartbeatBody>>,
) -> Result<Json<HeartbeatAck>, AppError> {
    auth.require(SCOPE_BRIDGE_HEARTBEAT)?;
    let bridge = db::bridges::find_by_id(&state.chat, bridge_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if bridge.bot_user_id != auth.user.id {
        return Err(AppError::Forbidden);
    }
    let mut error_owned;
    let error_ref: Option<&str> = match payload.and_then(|Json(p)| p.error) {
        Some(e) if !e.trim().is_empty() => {
            error_owned = e;
            if error_owned.len() > HEARTBEAT_ERROR_MAX_BYTES {
                error_owned.truncate(HEARTBEAT_ERROR_MAX_BYTES);
            }
            Some(error_owned.as_str())
        }
        _ => None,
    };
    db::bridges::record_heartbeat(&state.chat, bridge_id, error_ref).await?;
    Ok(Json(HeartbeatAck {
        ok: true,
        status: if error_ref.is_some() {
            "errored"
        } else {
            "healthy"
        },
    }))
}
