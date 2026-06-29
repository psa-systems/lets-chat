//! LC-495: workflow-automations config UI (the room manage page section).
//!
//! All three mutations re-authorize with `require_can_manage` and return the
//! re-rendered `#lc-automations` section so HTMX swaps it in place. The engine
//! that actually runs these rules lives in `crate::automations`; this module is
//! only CRUD over `db::automations`.

use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::room_automations::{AutomationRow, RoomAutomationsFragment};
use crate::views::{html, Html};

/// Cap on rules per room (keeps the manage list bounded and the post-path scan
/// cheap). Generous for legitimate use.
const MAX_RULES_PER_ROOM: i64 = 25;
const MAX_NAME_CHARS: usize = 80;
const MAX_MATCH_CHARS: usize = 200;

#[derive(Deserialize)]
pub struct CreateForm {
    #[serde(default)]
    pub name: String,
    pub trigger_kind: String,
    #[serde(default)]
    pub match_text: String,
    pub action_body: String,
}

#[derive(Deserialize)]
pub struct ToggleForm {
    pub enabled: String,
}

/// Re-render the automations section for `room_id` (the shared partial). Every
/// mutation returns this so the list stays current after the swap.
async fn render_section(state: &AppState, room_id: i64) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let automations: Vec<AutomationRow> = db::automations::list_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .map(AutomationRow::from_rule)
        .collect();
    html(&RoomAutomationsFragment {
        room: &room,
        automations: &automations,
    })
}

/// POST /room/{id}/automations - create a rule.
pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<CreateForm>,
) -> Result<Html, AppError> {
    super::room_rbac::require_can_manage(&state, &user, room_id).await?;

    let trigger = form.trigger_kind.trim();
    if !crate::automations::valid_trigger(trigger) {
        return Err(AppError::BadRequest("unknown trigger".into()));
    }
    let action_body = form.action_body.trim();
    if action_body.is_empty() {
        return Err(AppError::BadRequest(
            "the action message cannot be empty".into(),
        ));
    }
    if action_body.chars().count() > crate::routes::room::MAX_MESSAGE_CHARS {
        return Err(AppError::BadRequest(
            "the action message is too long".into(),
        ));
    }
    let name = form.name.trim();
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::BadRequest("name too long".into()));
    }
    let match_text = form.match_text.trim();
    if match_text.chars().count() > MAX_MATCH_CHARS {
        return Err(AppError::BadRequest("match text too long".into()));
    }

    if db::automations::count_for_room(&state.chat, room_id).await? >= MAX_RULES_PER_ROOM {
        return Err(AppError::BadRequest(format!(
            "a room can have at most {MAX_RULES_PER_ROOM} automations"
        )));
    }

    db::automations::insert(
        &state.chat,
        room_id,
        Some(name).filter(|s| !s.is_empty()),
        trigger,
        Some(match_text).filter(|s| !s.is_empty()),
        crate::automations::ACTION_POST_MESSAGE,
        action_body,
        &user.id,
    )
    .await?;

    db::moderation::log_mod_action(
        &state.chat,
        "room_automation_create",
        "",
        &user.id,
        Some(trigger),
        Some(room_id),
        None,
    )
    .await?;

    render_section(&state, room_id).await
}

/// POST /room/{id}/automations/{automation_id}/toggle - enable/disable a rule.
pub async fn post_toggle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, automation_id)): Path<(i64, i64)>,
    Form(form): Form<ToggleForm>,
) -> Result<Html, AppError> {
    super::room_rbac::require_can_manage(&state, &user, room_id).await?;
    let enabled = matches!(form.enabled.trim(), "1" | "true" | "on" | "yes");
    db::automations::set_enabled(&state.chat, automation_id, room_id, enabled).await?;
    render_section(&state, room_id).await
}

/// DELETE /room/{id}/automations/{automation_id} - remove a rule.
pub async fn delete_automation(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, automation_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    super::room_rbac::require_can_manage(&state, &user, room_id).await?;
    db::automations::delete(&state.chat, automation_id, room_id).await?;
    render_section(&state, room_id).await
}
