//! LC-62: HTTP routes for the scheduled-send management surface.
//!
//! - `GET /scheduled` renders the management page. Bodies are rendered
//!   through `views::markdown::render` against the current users table on
//!   every view, so mention chips whose target was renamed away show as
//!   literal text immediately (the Option B visible signal from the
//!   brainstorm). Mentions on dropped rows render the same way.
//! - `POST /scheduled` validates schedule-time inputs (lead-time bounds,
//!   attachment ownership, quote validity, plus the shared delivery
//!   gates) and inserts a row.
//! - `PATCH /scheduled/:id` and `DELETE /scheduled/:id` are user-scoped
//!   row operations. Both refuse to touch already-delivered rows and
//!   surface that as 410 Gone via a direct `Response` so the management
//!   UI can update its row state precisely.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::scheduled::{check_delivery_gates, DenyReason};
use crate::state::AppState;
use crate::views::scheduled::{DroppedRow, PendingRow, ScheduledPage, ScheduledRowFragment};
use crate::views::{html, Html};
use askama::Template;

/// Lead-time bounds for `scheduled_for`. Anything shorter than 30s
/// blurs the line between "scheduled" and "send now"; anything past
/// 365 days is a calendar reminder, not a chat message.
const MIN_LEAD_SECONDS: i64 = 30;
const MAX_LEAD_DAYS: i64 = 365;
/// Cap on dropped rows shown in the "Recently dropped" section. Older
/// rows stay in the table but are not surfaced; v1 has no operator UI
/// for inspecting beyond this window.
const RECENT_DROPPED_LIMIT: i64 = 20;

#[derive(Deserialize)]
pub struct CreateForm {
    pub room_id: i64,
    pub body: String,
    /// Caller submits UTC ISO8601 (e.g., from the composer modal's JS
    /// after converting `<input type="datetime-local">` to UTC). The
    /// route stores it in SQLite's `YYYY-MM-DD HH:MM:SS` shape for
    /// consistency with the existing chat datetime columns.
    pub scheduled_for: String,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub quote_id: Option<i64>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub file_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct UpdateForm {
    pub body: String,
    pub scheduled_for: String,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub file_id: Option<i64>,
}

fn empty_string_as_none<'de, D>(de: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<i64>().map(Some).map_err(serde::de::Error::custom),
    }
}

/// Parse RFC3339 UTC + range-check against the lead-time bounds. Returns
/// the SQLite storage-format string on success so callers do not have to
/// touch chrono types after the bounds check.
fn parse_and_check_scheduled_for(s: &str) -> Result<String, AppError> {
    let dt = DateTime::parse_from_rfc3339(s.trim())
        .map_err(|e| AppError::BadRequest(format!("invalid scheduled_for ({e})")))?
        .with_timezone(&Utc);
    let now = Utc::now();
    let delta = dt - now;
    if delta < chrono::Duration::seconds(MIN_LEAD_SECONDS) {
        return Err(AppError::BadRequest(format!(
            "scheduled_for must be at least {MIN_LEAD_SECONDS} seconds in the future"
        )));
    }
    if delta > chrono::Duration::days(MAX_LEAD_DAYS) {
        return Err(AppError::BadRequest(format!(
            "scheduled_for must be within {MAX_LEAD_DAYS} days"
        )));
    }
    Ok(dt.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn deny_reason_to_app_error(reason: DenyReason) -> AppError {
    match reason {
        // Link is a payload problem, not an authorization problem.
        DenyReason::LinkBlocked => AppError::BadRequest(reason.as_str().to_string()),
        _ => AppError::Forbidden,
    }
}

/// Schedule-time attachment + enclave quota check. Mirrors the live-send
/// path in `routes/room.rs::post_message` (the live path keeps its own
/// inline copy intentionally; if that changes, this must change too).
async fn validate_attachment(
    state: &AppState,
    user: &User,
    room_id: i64,
    file_id: i64,
) -> Result<(), AppError> {
    let (row, _) = db::uploads::get_upload(&state.chat, file_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("unknown file_id".into()))?;
    if row.uploader_id != user.id || row.message_id.is_some() {
        return Err(AppError::Forbidden);
    }
    if let Some(enclave_id) = db::quota::enclave_id_for_room(&state.chat, room_id).await? {
        if let Some(quota) = db::quota::get_enclave_quota(&state.chat, enclave_id).await? {
            let used = db::quota::sum_enclave_usage(&state.chat, enclave_id).await?;
            if used.saturating_add(row.size_bytes) > quota {
                return Err(AppError::PayloadTooLarge(format!(
                    "attachment would exceed the enclave storage quota \
                     ({quota} bytes; using {used}, this attachment is {})",
                    row.size_bytes
                )));
            }
        }
    }
    Ok(())
}

async fn validate_quote(state: &AppState, room_id: i64, quote_id: i64) -> Result<(), AppError> {
    let quoted = db::chat::get_message(&state.chat, quote_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("unknown quote_id".into()))?;
    if quoted.room_id != room_id {
        return Err(AppError::BadRequest(
            "quoted message is in a different room".into(),
        ));
    }
    if quoted.parent_id.is_some() {
        return Err(AppError::BadRequest(
            "cannot quote a thread reply; quote the thread root instead".into(),
        ));
    }
    Ok(())
}

/// Render `body` against the current users table so mention chips
/// reflect renames between schedule and delivery. Tokens that no longer
/// resolve become literal text; that is the Option B visible signal the
/// author sees on `/scheduled` before the dispatcher fires.
async fn render_body_for_management(state: &AppState, body: &str) -> String {
    let tokens = db::mentions::parse_mention_tokens(body);
    let mut mentions = Vec::new();
    for token in tokens {
        if let Ok(Some(record)) = db::auth::find_user_by_username(&state.auth, &token).await {
            mentions.push(db::mentions::MentionRef {
                user_id: record.id,
                username: record.username,
            });
        }
    }
    // Custom emojis are room-scoped; rendering them per row would be
    // an N+1 lookup against custom_emojis for what is a management
    // surface, not a chat surface. Empty slice ships the literal
    // `:shortcode:` text, which matches the simplest reading of "what
    // will the message look like" without expensive resolution.
    crate::views::markdown::render(body, &mentions, &[])
}

async fn build_pending_row(state: &AppState, row: &db::scheduled::ScheduledRow) -> PendingRow {
    let (room_label, room_path) = label_for_room(state, row.room_id).await;
    let body_html = render_body_for_management(state, &row.body).await;
    PendingRow {
        id: row.id,
        room_label,
        room_path,
        scheduled_for: row.scheduled_for.clone(),
        body_html,
    }
}

async fn build_dropped_row(state: &AppState, row: &db::scheduled::ScheduledRow) -> DroppedRow {
    let (room_label, _path) = label_for_room(state, row.room_id).await;
    let body_html = render_body_for_management(state, &row.body).await;
    DroppedRow {
        id: row.id,
        room_label,
        scheduled_for: row.scheduled_for.clone(),
        delivered_at: row.delivered_at.clone().unwrap_or_default(),
        failed_reason: row.failed_reason.clone().unwrap_or_default(),
        body_html,
    }
}

async fn label_for_room(state: &AppState, room_id: i64) -> (String, String) {
    let path = format!("/room/{room_id}");
    match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) => {
            let label = if r.room_type == "dm" {
                "Direct message".to_string()
            } else {
                format!("#{}", r.name)
            };
            (label, path)
        }
        _ => ("(room gone)".to_string(), path),
    }
}

/// GET /scheduled
pub async fn get_scheduled(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let pending_rows = db::scheduled::list_pending_for_user(&state.chat, &user.id).await?;
    let dropped_rows =
        db::scheduled::list_recent_dropped_for_user(&state.chat, &user.id, RECENT_DROPPED_LIMIT)
            .await?;

    let mut pending = Vec::with_capacity(pending_rows.len());
    for row in &pending_rows {
        pending.push(build_pending_row(&state, row).await);
    }
    let mut dropped = Vec::with_capacity(dropped_rows.len());
    for row in &dropped_rows {
        dropped.push(build_dropped_row(&state, row).await);
    }

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;

    let page = ScheduledPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        pending: &pending,
        dropped: &dropped,
        asset_version: &state.asset_version,
    };
    html(&page)
}

/// POST /scheduled
pub async fn post_scheduled(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateForm>,
) -> Result<Html, AppError> {
    let body = form.body.trim();
    if body.is_empty() && form.file_id.is_none() {
        return Err(AppError::BadRequest(
            "scheduled message body cannot be empty".into(),
        ));
    }

    // Rate limit: scheduled sends count against the same per-user cap
    // as live sends. Delivery does NOT re-check rate-limit by design.
    let msg_cap = crate::rate_limit::read_u32_setting(&state.settings, "rate_limit_messages").await;
    if let crate::rate_limit::Outcome::Deny { retry_after } =
        state
            .rate_limits
            .check(crate::rate_limit::RateLimitKind::Message, &user.id, msg_cap)
    {
        return Err(AppError::TooManyRequests(
            format!("scheduling messages too quickly; retry in {retry_after} seconds"),
            retry_after,
        ));
    }

    let room = db::chat::get_room(&state.chat, form.room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if let Some(reason) = check_delivery_gates(&state, &user, &room, body).await? {
        return Err(deny_reason_to_app_error(reason));
    }

    if let Some(file_id) = form.file_id {
        validate_attachment(&state, &user, room.id, file_id).await?;
    }

    if let Some(qid) = form.quote_id {
        validate_quote(&state, room.id, qid).await?;
    }

    let stored = parse_and_check_scheduled_for(&form.scheduled_for)?;

    let id = db::scheduled::insert_scheduled(
        &state.chat,
        room.id,
        &user.id,
        body,
        &stored,
        None,
        form.quote_id,
        form.file_id,
    )
    .await?;

    // LC-64: clear the composer's draft for this room now that the
    // text is safely captured in scheduled_messages. Best-effort; a
    // DB hiccup must not fail the schedule. The dispatcher's
    // eventual delivery path goes through finalize_message_send,
    // which also clears the draft - the redundancy is harmless and
    // matches the "clear at every commit point" posture.
    // LC-239: clear the sidebar draft indicator across the user's tabs when
    // a draft actually existed. Gated on rows_affected to avoid a redundant
    // OOB fan when scheduling without a saved draft.
    if db::drafts::delete(&state.chat, &user.id, room.id)
        .await
        .unwrap_or(0)
        > 0
    {
        state.hub.broadcast_to_user(
            &user.id,
            &crate::ws::events::ChatEvent::DraftChanged {
                user_id: user.id.clone(),
                room_id: room.id,
                has_draft: false,
            },
        );
    }

    let success = format!(
        "<div class=\"text-sm text-green-700\" role=\"status\" \
         data-scheduled-id=\"{id}\">Scheduled for {stored} UTC.</div>"
    );
    Ok(Html(success))
}

/// PATCH /scheduled/:id
pub async fn patch_scheduled(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Form(form): Form<UpdateForm>,
) -> Result<Response, AppError> {
    let existing = db::scheduled::get_scheduled(&state.chat, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if existing.user_id != user.id {
        return Err(AppError::Forbidden);
    }
    if existing.delivered_at.is_some() {
        return Ok((StatusCode::GONE, "already delivered or dropped").into_response());
    }

    let body = form.body.trim();
    if body.is_empty() && form.file_id.is_none() {
        return Err(AppError::BadRequest(
            "scheduled message body cannot be empty".into(),
        ));
    }

    let room = db::chat::get_room(&state.chat, existing.room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if let Some(reason) = check_delivery_gates(&state, &user, &room, body).await? {
        return Err(deny_reason_to_app_error(reason));
    }

    if let Some(file_id) = form.file_id {
        // Allow re-binding the same already-linked file_id (idempotent
        // re-submit of an edit); only fully-new file_ids go through the
        // ownership check.
        if Some(file_id) != existing.file_id {
            validate_attachment(&state, &user, room.id, file_id).await?;
        }
    }

    let stored = parse_and_check_scheduled_for(&form.scheduled_for)?;

    let updated =
        db::scheduled::update_scheduled(&state.chat, id, &user.id, body, &stored, form.file_id)
            .await?;
    if !updated {
        // Lost the race with the dispatcher: row got delivered between
        // our get_scheduled and the UPDATE.
        return Ok((StatusCode::GONE, "already delivered or dropped").into_response());
    }

    let refreshed =
        db::scheduled::get_scheduled(&state.chat, id)
            .await?
            .ok_or(AppError::Internal(
                "scheduled row vanished after successful update".into(),
            ))?;
    let pending = build_pending_row(&state, &refreshed).await;
    let fragment = ScheduledRowFragment { row: &pending };
    let body_html = fragment.render()?;
    Ok(Html(body_html).into_response())
}

/// DELETE /scheduled/:id
pub async fn delete_scheduled(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let existing = db::scheduled::get_scheduled(&state.chat, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if existing.user_id != user.id {
        return Err(AppError::Forbidden);
    }
    if existing.delivered_at.is_some() {
        return Ok((StatusCode::GONE, "already delivered or dropped").into_response());
    }
    let removed = db::scheduled::delete_scheduled(&state.chat, id, &user.id).await?;
    if !removed {
        return Ok((StatusCode::GONE, "already delivered or dropped").into_response());
    }
    // Empty body + hx-swap="delete" removes the row from the DOM.
    Ok(StatusCode::OK.into_response())
}
