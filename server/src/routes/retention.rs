//! Per-room message retention: admin-facing UI for the destructive
//! sweep.
//!
//! The sweep itself lives in `crate::retention::sweep` and ships behind
//! `LETS_CHAT_RETENTION_SWEEP_ENABLED`. This module is the user-facing
//! surface that admins use to opt a room in. Two endpoints:
//!
//! - `GET /room/{id}/retention/preview?days=N` returns an HTMX fragment
//!   with the count of messages the sweep would delete at that setting,
//!   plus the permanence warnings and an embedded "Confirm" form.
//! - `POST /room/{id}/retention` validates, writes `rooms.retention_days`,
//!   audits the change to `mod_actions`, and redirects back to the
//!   moderators page (where the retention section now reflects the new
//!   state).
//!
//! Both gates: `require_can_manage` (same admin/enclave-owner set that
//! controls posting policy and override grants) plus an explicit
//! `room_type != 'dm'` reject (defense-in-depth: the sweep predicate
//! already excludes DMs at the SQL level, this gate keeps the UI
//! coherent).

#[allow(unused_imports)]
use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::routes::room_rbac::require_can_manage;
use crate::state::AppState;
use crate::{db, retention};

#[derive(Deserialize)]
pub struct PreviewQuery {
    /// Empty / missing = preview a "disable retention" action. Numeric
    /// = preview "set retention_days to N". Validated against the
    /// 1-day floor that POST also enforces.
    pub days: Option<String>,
}

#[derive(Template)]
#[template(path = "room/retention_preview.html")]
struct RetentionPreviewFragment {
    room_id: i64,
    old_days: Option<i64>,
    /// `None` = the form is asking to disable retention; the fragment
    /// renders a "disabling does not undelete" notice instead of a
    /// count. `Some(N)` = enabling/changing; the fragment renders the
    /// count and permanence warning.
    new_days: Option<i64>,
    /// Only set when `new_days.is_some()`. Computed via
    /// `retention::sweep::count_candidates_for_room` so the displayed
    /// count agrees with what the sweep would actually delete; the
    /// `preview_count_equals_sweep_actual_delete` test in
    /// `tests/retention_sweep.rs` enforces this shared-predicate
    /// invariant at the SQL level.
    will_delete: Option<i64>,
}

/// `GET /room/{room_id}/retention/preview?days=N`
pub async fn get_preview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Query(q): Query<PreviewQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_can_manage(&state, &user, room_id).await?;
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "dm" {
        return Err(AppError::BadRequest(
            "retention is not configurable on DM rooms".into(),
        ));
    }
    let old_days = db::chat::get_room_retention_days(&state.chat, room_id).await?;
    let new_days = parse_days(q.days.as_deref())?;

    let will_delete = match new_days {
        Some(n) => {
            Some(retention::sweep::count_candidates_for_room(&state.chat, room_id, n).await?)
        }
        None => None,
    };

    let fragment = RetentionPreviewFragment {
        room_id,
        old_days,
        new_days,
        will_delete,
    };
    Ok(Html(fragment.render()?))
}

#[derive(Deserialize)]
pub struct SetForm {
    /// Empty / missing = disable retention on this room. Numeric =
    /// new `retention_days` value.
    pub days: Option<String>,
}

/// `POST /room/{room_id}/retention` form: days=N (or empty = disable).
///
/// Writes `rooms.retention_days`, records a `mod_actions` row with
/// `target_user='-'` (no target user; the target is the room setting),
/// `action='retention_set'`, and `metadata={"old_days":..,"new_days":..}`,
/// then redirects to the moderators page so the retention section
/// re-renders with the new state.
pub async fn post_set(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<SetForm>,
) -> Result<impl IntoResponse, AppError> {
    require_can_manage(&state, &user, room_id).await?;
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if room.room_type == "dm" {
        return Err(AppError::BadRequest(
            "retention is not configurable on DM rooms".into(),
        ));
    }
    let old_days = db::chat::get_room_retention_days(&state.chat, room_id).await?;
    let new_days = parse_days(form.days.as_deref())?;

    let n = db::chat::set_room_retention_days(&state.chat, room_id, new_days).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }

    let metadata = serde_json::json!({
        "old_days": old_days,
        "new_days": new_days,
    })
    .to_string();
    db::moderation::log_mod_action(
        &state.chat,
        "retention_set",
        "-",
        &user.id,
        None,
        Some(room_id),
        Some(&metadata),
    )
    .await?;

    Ok(Redirect::to(&format!("/room/{room_id}/moderators")))
}

/// Parse the days field from a form or query string.
///
/// - `None` or empty/whitespace -> `Ok(None)`, meaning "disable retention".
/// - A positive integer -> `Ok(Some(n))`.
/// - `0`, negative, or non-numeric -> `Err(AppError::BadRequest)`.
///
/// The `>= 1` floor here mirrors the schema CHECK constraint on
/// `rooms.retention_days` (migration 0043). The floor exists to prevent
/// an admin from fat-fingering `0` into a "delete everything"
/// instruction.
fn parse_days(raw: Option<&str>) -> Result<Option<i64>, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => {
            let n: i64 = s
                .parse()
                .map_err(|_| AppError::BadRequest("days must be an integer".into()))?;
            if n < 1 {
                return Err(AppError::BadRequest(
                    "days must be 1 or greater (submit empty to disable)".into(),
                ));
            }
            Ok(Some(n))
        }
    }
}
