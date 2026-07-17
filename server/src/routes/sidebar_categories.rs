//! HTMX endpoints for per-enclave room categorization (LC-79 redesign).
//!
//! All mutations require enclave admin / owner (or site admin) RBAC via
//! `enclave_can_manage`. The per-user collapsed toggle is the exception
//! (no admin RBAC, any enclave member may fold their own view) and
//! lives under a path that omits the enclave id.
//!
//! Each mutation re-renders the full sidebar partial via
//! `render_sidebar_fragment`, which the client swaps over the existing
//! `#sidebar` element. Mutations to shared category state additionally
//! broadcast a `SidebarCategoriesChanged` event to every member of the
//! enclave so other members' open tabs pick up the change live.
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::perms::enclave_can_manage;
use crate::state::AppState;
use crate::views::ws_fragments::SidebarUpdateFragment;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

const MAX_CATEGORY_NAME_LEN: usize = 50;

#[derive(Deserialize)]
pub struct CreateCategoryForm {
    pub name: String,
}

#[derive(Deserialize)]
pub struct RenameCategoryForm {
    pub name: String,
}

#[derive(Deserialize, Default)]
pub struct CollapseForm {
    /// Checkbox semantics: present -> collapsed = true.
    pub collapsed: Option<String>,
}

#[derive(Deserialize)]
pub struct PositionsForm {
    #[serde(default)]
    pub ids: String,
}

impl PositionsForm {
    fn parsed_ids(&self) -> Result<Vec<i64>, AppError> {
        self.ids
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i64>()
                    .map_err(|_| AppError::BadRequest(format!("invalid id: {s}")))
            })
            .collect()
    }
}

fn validate_name(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("category name cannot be empty".into()));
    }
    if trimmed.chars().count() > MAX_CATEGORY_NAME_LEN {
        return Err(AppError::BadRequest(format!(
            "category name longer than {MAX_CATEGORY_NAME_LEN} characters"
        )));
    }
    Ok(trimmed.to_string())
}

/// Enforce enclave admin / owner (or site admin) for `user` on
/// `enclave_id`. Guard on every mutation that changes shared category
/// state.
async fn require_enclave_manage(
    state: &AppState,
    user: &User,
    enclave_id: i64,
) -> Result<(), AppError> {
    let membership = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
    if !enclave_can_manage(membership.map(|m| m.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// POST /enclave/{id}/sidebar/categories
pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(enclave_id): Path<i64>,
    Form(form): Form<CreateCategoryForm>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    let name = validate_name(&form.name)?;
    db::sidebar_categories::create_category(&state.chat, enclave_id, &name).await?;
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// PATCH /enclave/{id}/sidebar/categories/{cat_id}
pub async fn patch_category(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, category_id)): Path<(i64, i64)>,
    Form(form): Form<RenameCategoryForm>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    let validated = validate_name(&form.name)?;
    let n =
        db::sidebar_categories::rename_category(&state.chat, enclave_id, category_id, &validated)
            .await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// DELETE /enclave/{id}/sidebar/categories/{cat_id}
pub async fn delete_category(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, category_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    let n = db::sidebar_categories::delete_category(&state.chat, enclave_id, category_id).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// PATCH /enclave/{id}/sidebar/categories/{cat_id}/rooms/{room_id}
pub async fn patch_room_assignment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, category_id, room_id)): Path<(i64, i64, i64)>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    assert_category_in_enclave(&state, enclave_id, category_id).await?;
    assert_room_in_enclave(&state, enclave_id, room_id).await?;
    db::sidebar_categories::assign_room(&state.chat, room_id, category_id).await?;
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// DELETE /enclave/{id}/sidebar/categories/rooms/{room_id}
pub async fn delete_room_assignment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, room_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    assert_room_in_enclave(&state, enclave_id, room_id).await?;
    db::sidebar_categories::unassign_room(&state.chat, room_id).await?;
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// PATCH /enclave/{id}/sidebar/categories/positions
pub async fn patch_category_positions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(enclave_id): Path<i64>,
    Form(form): Form<PositionsForm>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    let ids = form.parsed_ids()?;
    db::sidebar_categories::set_category_positions(&state.chat, enclave_id, &ids).await?;
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// PATCH /enclave/{id}/sidebar/categories/{cat_id}/positions
pub async fn patch_room_positions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, category_id)): Path<(i64, i64)>,
    Form(form): Form<PositionsForm>,
) -> Result<Html, AppError> {
    require_enclave_manage(&state, &user, enclave_id).await?;
    assert_category_in_enclave(&state, enclave_id, category_id).await?;
    let ids = form.parsed_ids()?;
    for room_id in &ids {
        assert_room_in_enclave(&state, enclave_id, *room_id).await?;
    }
    db::sidebar_categories::set_room_positions(&state.chat, category_id, &ids).await?;
    broadcast_refresh(&state, enclave_id).await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

/// PATCH /sidebar/categories/{cat_id}/collapse - per-user fold/unfold.
/// Any enclave member may toggle their own view; no admin RBAC.
pub async fn patch_collapse(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(category_id): Path<i64>,
    Form(form): Form<CollapseForm>,
) -> Result<Html, AppError> {
    let enclave_id = db::sidebar_categories::enclave_of_category(&state.chat, category_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let membership = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
    let is_site_admin = user.role == "admin";
    if membership.is_none() && !is_site_admin {
        return Err(AppError::Forbidden);
    }
    db::sidebar_categories::set_collapsed(
        &state.auth,
        &user.id,
        category_id,
        form.collapsed.is_some(),
    )
    .await?;
    render_sidebar_with_enclave(&state, &user, Some(enclave_id)).await
}

async fn assert_category_in_enclave(
    state: &AppState,
    enclave_id: i64,
    category_id: i64,
) -> Result<(), AppError> {
    let parent = db::sidebar_categories::enclave_of_category(&state.chat, category_id).await?;
    if parent != Some(enclave_id) {
        return Err(AppError::NotFound);
    }
    Ok(())
}

async fn assert_room_in_enclave(
    state: &AppState,
    enclave_id: i64,
    room_id: i64,
) -> Result<(), AppError> {
    let room_enclave = super::enclave_for_room(state, room_id).await?;
    if room_enclave != Some(enclave_id) {
        return Err(AppError::NotFound);
    }
    Ok(())
}

// LC-250: reused by `routes::read_all` to return the re-rendered sidebar after
// "mark all as read", enclave-aware via the HX-Current-URL header. Used only by
// routes that have NO enclave in their own path (read-all, mark-room-read);
// the enclave-scoped category mutations below call
// `render_sidebar_with_enclave` with their authoritative path `enclave_id`
// instead - see that function's note for why the header is not enough.
pub(crate) async fn render_sidebar_fragment(
    state: &AppState,
    user: &User,
    headers: &HeaderMap,
) -> Result<Html, AppError> {
    let current_enclave = super::current_enclave_for_sidebar(state, headers).await;
    render_sidebar_with_enclave(state, user, current_enclave).await
}

/// Re-render the whole-sidebar OOB fragment for `user` in an explicit enclave
/// context.
///
/// The enclave-scoped category mutations pass their authoritative path
/// `enclave_id` here rather than re-deriving the current enclave from request
/// headers (`super::current_enclave_for_sidebar`). The URL fallback in that
/// resolver only understands `/enclave/{id}` and `/room/{id}` URLs and returns
/// `None` for anything else (or if the header is absent / stripped by a proxy /
/// a WS swap raced the submit). A `None` re-render collapses the sidebar to its
/// DM-only shape - which has no categories and no add-category form - so the
/// just-created category and the input both vanished and the user saw "nothing
/// happened" (LC-415). The path `enclave_id` is the enclave the form was
/// rendered in, so it is always the correct context for the re-render.
pub(crate) async fn render_sidebar_with_enclave(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_sidebar(state, user, current_enclave).await?;
    let switcher = super::load_switcher(state, user, sidebar_current_enclave).await?;
    let fragment = SidebarUpdateFragment {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
    };
    html(&fragment)
}

async fn broadcast_refresh(state: &AppState, enclave_id: i64) -> Result<(), AppError> {
    let members = db::enclave::list_members(&state.chat, enclave_id).await?;
    let event = ChatEvent::SidebarCategoriesChanged { enclave_id };
    for m in members {
        state.hub.broadcast_to_user(&m.user_id, &event);
    }
    Ok(())
}
