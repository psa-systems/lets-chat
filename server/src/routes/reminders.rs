//! LC-63: HTTP routes for message reminders.
//!
//! - `GET /reminders` lists the caller's pending + recently-fired reminders.
//! - `GET /reminders/picker?message_id=` returns the "Remind me" popover for
//!   the message hover-menu.
//! - `POST /reminders` inserts a reminder (relative preset or custom UTC
//!   time) and returns an inline confirmation.
//! - `DELETE /reminders/{id}` cancels a pending reminder.
//!
//! Reminders are private: every query is scoped to the caller's id, and a
//! reminder can only be set on a message in a room the caller can see.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::reminders::{ReminderConfirm, ReminderListRow, ReminderPicker, RemindersPage};
use crate::views::{html, Html};

/// Cap on the "recently fired" list shown on the page.
const RECENT_FIRED_LIMIT: i64 = 25;
/// A custom time must be at least this far out (anything less is "now").
const MIN_LEAD_SECONDS: i64 = 30;
/// And no further than this (a reminder, not a calendar event).
const MAX_LEAD_DAYS: i64 = 365;

#[derive(Deserialize)]
pub struct PickerQuery {
    pub message_id: i64,
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub message_id: i64,
    /// One of "15m" | "1h" | "3h" | "tomorrow". Mutually exclusive with
    /// `remind_at`.
    #[serde(default)]
    pub preset: Option<String>,
    /// Custom time as UTC RFC3339 (the picker's JS converts the
    /// `datetime-local` value). Used when `preset` is absent.
    #[serde(default)]
    pub remind_at: Option<String>,
}

async fn room_id_for_message(state: &AppState, message_id: i64) -> Result<i64, AppError> {
    db::reminders::message_room(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)
}

async fn require_room_access(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Resolve the requested reminder time to SQLite's UTC storage string.
fn resolve_remind_at(form: &CreateForm) -> Result<String, AppError> {
    if let Some(p) = form.preset.as_deref().filter(|s| !s.is_empty()) {
        let dur = match p {
            "15m" => Duration::minutes(15),
            "1h" => Duration::hours(1),
            "3h" => Duration::hours(3),
            "tomorrow" => Duration::hours(24),
            _ => return Err(AppError::BadRequest("unknown reminder preset".into())),
        };
        let dt = Utc::now() + dur;
        return Ok(dt.format("%Y-%m-%d %H:%M:%S").to_string());
    }
    if let Some(c) = form.remind_at.as_deref().filter(|s| !s.is_empty()) {
        let dt = DateTime::parse_from_rfc3339(c.trim())
            .map_err(|e| AppError::BadRequest(format!("invalid remind_at ({e})")))?
            .with_timezone(&Utc);
        let delta = dt - Utc::now();
        if delta < Duration::seconds(MIN_LEAD_SECONDS) {
            return Err(AppError::BadRequest(
                "reminder time must be in the future".into(),
            ));
        }
        if delta > Duration::days(MAX_LEAD_DAYS) {
            return Err(AppError::BadRequest(format!(
                "reminder time must be within {MAX_LEAD_DAYS} days"
            )));
        }
        return Ok(dt.format("%Y-%m-%d %H:%M:%S").to_string());
    }
    Err(AppError::BadRequest("pick a reminder time".into()))
}

/// Build the deep link + label for one reminder row. DMs resolve the peer.
async fn context_for(
    state: &AppState,
    user_id: &str,
    row: &db::reminders::ReminderRow,
) -> (String, String) {
    if row.room_type == "dm" {
        let peer = db::chat::get_dm_peer(&state.chat, row.room_id, user_id)
            .await
            .ok()
            .flatten();
        match peer {
            Some(p) => (
                "Direct message".to_string(),
                format!("/dm/{p}#msg-{}", row.message_id),
            ),
            None => (
                "Direct message".to_string(),
                format!("/room/{}#msg-{}", row.room_id, row.message_id),
            ),
        }
    } else {
        (
            format!("#{}", row.room_name),
            format!("/room/{}#msg-{}", row.room_id, row.message_id),
        )
    }
}

async fn to_list_row(
    state: &AppState,
    user_id: &str,
    row: db::reminders::ReminderRow,
) -> ReminderListRow {
    let (context_label, context_path) = context_for(state, user_id, &row).await;
    ReminderListRow {
        id: row.id,
        snippet: row.snippet,
        remind_at: row.remind_at,
        fired_at: row.fired_at,
        context_label,
        context_path,
    }
}

/// GET /reminders
pub async fn get_reminders(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let pending_raw = db::reminders::list_pending_for_user(&state.chat, &user.id).await?;
    let fired_raw =
        db::reminders::list_recent_fired_for_user(&state.chat, &user.id, RECENT_FIRED_LIMIT)
            .await?;
    let mut pending = Vec::with_capacity(pending_raw.len());
    for r in pending_raw {
        pending.push(to_list_row(&state, &user.id, r).await);
    }
    let mut fired = Vec::with_capacity(fired_raw.len());
    for r in fired_raw {
        fired.push(to_list_row(&state, &user.id, r).await);
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
    let page = RemindersPage {
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
        fired: &fired,
        asset_version: &state.asset_version,
    };
    html(&page)
}

/// GET /reminders/picker?message_id=
pub async fn get_picker(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<PickerQuery>,
) -> Result<Html, AppError> {
    let room_id = room_id_for_message(&state, q.message_id).await?;
    require_room_access(&state, &user, room_id).await?;
    html(&ReminderPicker {
        message_id: q.message_id,
    })
}

/// POST /reminders
pub async fn post_reminder(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    let room_id = room_id_for_message(&state, form.message_id).await?;
    require_room_access(&state, &user, room_id).await?;
    let remind_at = resolve_remind_at(&form)?;
    db::reminders::insert(&state.chat, &user.id, form.message_id, &remind_at).await?;
    Ok(html(&ReminderConfirm { remind_at })?.into_response())
}

/// DELETE /reminders/{id}
pub async fn delete_reminder(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let removed = db::reminders::delete(&state.chat, id, &user.id).await?;
    if !removed {
        return Err(AppError::NotFound);
    }
    // hx-swap="delete" on the caller removes the row; empty 200 body.
    Ok((StatusCode::OK, "").into_response())
}
