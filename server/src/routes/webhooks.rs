//! LC-74: incoming webhooks. The public `POST /webhook/{secret}` route is
//! unauthenticated (the secret in the URL is the credential) and is merged
//! AFTER the TraceLayer so the secret never lands in request logs. The
//! room-scoped management routes are cookie-authenticated and gated to room
//! moderators.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::webhooks::{RoomWebhooksPage, WebhookRowView};
use crate::views::{html, Html};

/// Fixed per-webhook rate cap (requests/minute). A misbehaving external
/// system cannot flood a room beyond this.
const WEBHOOK_RATE_PER_MIN: u32 = 60;

/// Cap on a single webhook message's length (bytes). Generous for alerts
/// while bounding what one unauthenticated POST can push into a room.
const WEBHOOK_MAX_TEXT_BYTES: usize = 16 * 1024;

/// Public router for the unauthenticated incoming endpoint. Merged after the
/// TraceLayer in `build_router` so the secret is never logged.
pub fn public_router() -> Router<AppState> {
    Router::new().route("/webhook/{secret}", post(post_incoming))
}

#[derive(Deserialize)]
pub struct IncomingPayload {
    /// Message text. (Slack/Discord-compatible field name.)
    pub text: String,
    /// When true, the text is rendered through the markdown pipeline;
    /// otherwise it is escaped so it renders literally. Default false.
    #[serde(default)]
    pub markdown: bool,
}

/// Escape markdown metacharacters so a plaintext payload renders literally
/// through the shared markdown pipeline.
fn escape_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '>'
                | '~'
                | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// POST /webhook/{secret} - append a message to the webhook's room.
pub async fn post_incoming(
    State(state): State<AppState>,
    Path(secret): Path<String>,
    Json(payload): Json<IncomingPayload>,
) -> Result<Response, AppError> {
    // Webhooks require a secret key to hash the credential.
    let Some(key) = state.secret_key.as_ref() else {
        return Err(AppError::NotFound);
    };
    let hash = crate::auth::hash_api_token(key, &secret);
    let Some(hook) = db::webhooks::find_by_secret_hash(&state.chat, &hash).await? else {
        // Unknown secret: 401 without revealing anything.
        return Err(AppError::Unauthorized);
    };
    if hook.revoked_at.is_some() {
        // Gone: the webhook was revoked.
        return Ok(StatusCode::GONE.into_response());
    }

    // Per-webhook rate limit -> 429 + Retry-After.
    if let crate::rate_limit::Outcome::Deny { retry_after } = state.rate_limits.check(
        crate::rate_limit::RateLimitKind::Webhook,
        &hook.id.to_string(),
        WEBHOOK_RATE_PER_MIN,
    ) {
        return Err(AppError::TooManyRequests(
            "webhook rate limit exceeded".into(),
            retry_after,
        ));
    }

    let text = payload.text.trim();
    if text.is_empty() {
        return Err(AppError::BadRequest("text cannot be empty".into()));
    }
    if text.len() > WEBHOOK_MAX_TEXT_BYTES {
        return Err(AppError::BadRequest("text too long".into()));
    }
    let body = if payload.markdown {
        text.to_string()
    } else {
        escape_markdown(text)
    };

    let room = db::chat::get_room(&state.chat, hook.room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let new_id =
        db::chat::insert_webhook_message(&state.chat, hook.room_id, hook.id, &body).await?;
    super::room::finalize_webhook_message_send(&state, &room, hook.id, &hook.name, new_id, &body)
        .await?;

    // Best-effort last_used bump; never block the request.
    let pool = state.chat.clone();
    let id = hook.id;
    tokio::spawn(async move {
        let _ = db::webhooks::touch_last_used(&pool, id).await;
    });

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ── room-scoped management (cookie-authed, moderator-gated) ─────────────

async fn require_room_manage(
    state: &AppState,
    user: &crate::models::User,
    room_id: i64,
) -> Result<(), AppError> {
    if db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn render_page(
    state: &AppState,
    user: &crate::models::User,
    room_id: i64,
    new_url: Option<String>,
    error: Option<String>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let current_enclave = super::enclave_for_room(state, room_id).await?;
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, current_enclave).await?;
    let webhooks: Vec<WebhookRowView> = db::webhooks::list_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .map(|w| WebhookRowView {
            id: w.id,
            name: w.name,
            avatar_url: w.avatar_url,
            created_at: w.created_at,
            last_used_at: w.last_used_at,
            revoked: w.revoked,
        })
        .collect();
    html(&RoomWebhooksPage {
        user,
        room: &room,
        available: state.secret_key.is_some(),
        webhooks: &webhooks,
        new_url,
        error,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}

/// GET /room/{id}/webhooks
pub async fn get_webhooks(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    render_page(&state, &user, room_id, None, None).await
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub name: String,
    #[serde(default)]
    pub avatar_url: String,
}

/// POST /room/{id}/webhooks - create a webhook and reveal its URL once.
pub async fn post_webhooks(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<Response, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    let Some(key) = state.secret_key.as_ref() else {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some("Webhooks need a server secret key (LETS_CHAT_SECRET_KEY).".into()),
        )
        .await?
        .into_response());
    };
    let name = form.name.trim();
    if name.is_empty() {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some("Webhook name is required.".into()),
        )
        .await?
        .into_response());
    }
    let avatar = form.avatar_url.trim();
    let avatar = (!avatar.is_empty()).then_some(avatar);
    if let Some(a) = avatar {
        if !(a.starts_with("https://") || a.starts_with("http://")) {
            return Ok(render_page(
                &state,
                &user,
                room_id,
                None,
                Some("Avatar URL must be an http(s) URL.".into()),
            )
            .await?
            .into_response());
        }
    }

    let secret = crate::auth::generate_api_token();
    let hash = crate::auth::hash_api_token(key, &secret);
    db::webhooks::insert(&state.chat, room_id, name, avatar, &hash, &user.id).await?;
    let url = format!("{}/webhook/{}", state.base_url, secret);
    Ok(render_page(&state, &user, room_id, Some(url), None)
        .await?
        .into_response())
}

/// POST /room/{id}/webhooks/{wid}/revoke
pub async fn post_revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, wid)): Path<(i64, i64)>,
) -> Result<Redirect, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    db::webhooks::revoke(&state.chat, wid, room_id).await?;
    Ok(Redirect::to(&format!("/room/{room_id}/webhooks")))
}
