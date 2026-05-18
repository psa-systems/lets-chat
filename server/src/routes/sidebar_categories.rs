//! HTMX endpoints for per-user sidebar categorization.
//!
//! Each mutation re-renders the full sidebar partial via
//! `render_sidebar_fragment`, which the client swaps over the existing
//! `#sidebar` element. Keeps the response shape uniform and avoids
//! per-endpoint OOB swap plumbing for what is a small navigation surface.
use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::ws_fragments::SidebarUpdateFragment;
use crate::views::{html, Html};

/// Category name length: matches the practical sidebar width. 50 chars is
/// plenty for "Work Projects (Q3)" and refuses absurd input.
const MAX_CATEGORY_NAME_LEN: usize = 50;

#[derive(Deserialize)]
pub struct CreateCategoryForm {
    pub name: String,
}

#[derive(Deserialize, Default)]
pub struct PatchCategoryForm {
    pub name: Option<String>,
    /// Checkbox semantics: present (any value) -> collapsed = true.
    pub collapsed: Option<String>,
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

/// POST /sidebar/categories - create a new category for the calling user.
pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateCategoryForm>,
) -> Result<Html, AppError> {
    let name = validate_name(&form.name)?;
    db::sidebar_categories::create_category(&state.auth, &user.id, &name).await?;
    render_sidebar_fragment(&state, &user).await
}

/// PATCH /sidebar/categories/{id} - rename or toggle collapsed. The
/// request can carry either field independently; the form decoder gives
/// Option<String> for both.
pub async fn patch_category(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(category_id): Path<i64>,
    Form(form): Form<PatchCategoryForm>,
) -> Result<Html, AppError> {
    if let Some(name) = form.name.as_deref() {
        let validated = validate_name(name)?;
        let n =
            db::sidebar_categories::rename_category(&state.auth, &user.id, category_id, &validated)
                .await?;
        if n == 0 {
            return Err(AppError::NotFound);
        }
    }
    // The collapsed field is only meaningful when explicitly present:
    // an absent checkbox means "uncollapse", an "on" value means
    // "collapse". Both states get persisted; passing neither name nor
    // collapsed is a no-op patch.
    let collapsed_present = form.collapsed.is_some() || form.name.is_none();
    if collapsed_present {
        let collapsed = form.collapsed.is_some();
        let n =
            db::sidebar_categories::set_collapsed(&state.auth, &user.id, category_id, collapsed)
                .await?;
        if n == 0 && form.name.is_none() {
            return Err(AppError::NotFound);
        }
    }
    render_sidebar_fragment(&state, &user).await
}

/// DELETE /sidebar/categories/{id} - remove a category. Rooms previously
/// assigned to it fall back to the "All rooms" bucket via the FK CASCADE
/// on `sidebar_category_rooms`.
pub async fn delete_category(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(category_id): Path<i64>,
) -> Result<Html, AppError> {
    let n = db::sidebar_categories::delete_category(&state.auth, &user.id, category_id).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    render_sidebar_fragment(&state, &user).await
}

/// PATCH /sidebar/categories/{id}/rooms/{room_id} - assign a room to a
/// category. The caller must be a member of the room (checked here so
/// users cannot use the categorization endpoint to disclose private-room
/// existence).
pub async fn patch_room_assignment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((category_id, room_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    // Confirm the target category belongs to the user before touching the
    // assignment row; rename / set_collapsed enforce this implicitly via
    // their WHERE clause but assign_room does not.
    let categories = db::sidebar_categories::list_categories(&state.auth, &user.id).await?;
    if !categories.iter().any(|c| c.id == category_id) {
        return Err(AppError::NotFound);
    }
    db::sidebar_categories::assign_room(&state.auth, &user.id, room_id, category_id).await?;
    render_sidebar_fragment(&state, &user).await
}

/// DELETE /sidebar/categories/rooms/{room_id} - unassign a room (move it
/// back to the uncategorized "All rooms" bucket).
pub async fn delete_room_assignment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    db::sidebar_categories::unassign_room(&state.auth, &user.id, room_id).await?;
    render_sidebar_fragment(&state, &user).await
}

/// Form shape for the two positions endpoints. The body is a single
/// comma-separated `ids` field (eg. `ids=3,1,2`) so the standard
/// `application/x-www-form-urlencoded` codec serde-urlencoded uses can
/// decode it. The repeated-key shape (`ids[]=1&ids[]=2`) is not
/// supported by axum's default Form extractor.
#[derive(Deserialize)]
pub struct PositionsForm {
    #[serde(default)]
    pub ids: String,
}

impl PositionsForm {
    /// Parse the comma-separated id list into a `Vec<i64>`. Skips empty
    /// entries so a leading / trailing / repeated comma in the request
    /// body is tolerated; surfaces a 400 for any non-integer entry so
    /// the client gets a clear error rather than a silent reorder skip.
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

/// PATCH /sidebar/categories/positions - reorder all categories owned
/// by the caller. Body carries the new ordering as
/// `ids=<cat_id>,<cat_id>,...`.
pub async fn patch_category_positions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<PositionsForm>,
) -> Result<Html, AppError> {
    let ids = form.parsed_ids()?;
    db::sidebar_categories::set_category_positions(&state.auth, &user.id, &ids).await?;
    render_sidebar_fragment(&state, &user).await
}

/// PATCH /sidebar/categories/{cat_id}/positions - reorder the rooms in
/// one category. The body is the new ordering of room ids; rooms that
/// were previously in a different category get reassigned to this one
/// (the cross-category drag case). Each room id is membership-checked
/// against `chat.db` first so the endpoint cannot disclose private-room
/// existence.
pub async fn patch_room_positions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(category_id): Path<i64>,
    Form(form): Form<PositionsForm>,
) -> Result<Html, AppError> {
    let ids = form.parsed_ids()?;
    // Confirm the target category belongs to the user up-front so a
    // spoofed id from the form does not write rows under someone else's
    // category.
    let categories = db::sidebar_categories::list_categories(&state.auth, &user.id).await?;
    if !categories.iter().any(|c| c.id == category_id) {
        return Err(AppError::NotFound);
    }
    let is_admin = user.role == "admin";
    for room_id in &ids {
        if !db::chat::is_room_accessible(&state.chat, *room_id, &user.id, is_admin).await? {
            return Err(AppError::Forbidden);
        }
    }
    db::sidebar_categories::set_room_positions(&state.auth, &user.id, category_id, &ids).await?;
    render_sidebar_fragment(&state, &user).await
}

/// Re-render the sidebar partial. Uses `SidebarUpdateFragment` (the same
/// shape used for WebSocket-driven sidebar updates) so HTMX swaps the
/// `#sidebar` element via the `hx-swap-oob` attribute baked into
/// `ws/sidebar_update.html`. Phase 1b: now includes the grouped
/// `sidebar_categories` field so the rebuilt sidebar reflects the
/// mutation immediately.
async fn render_sidebar_fragment(state: &AppState, user: &User) -> Result<Html, AppError> {
    let (sidebar_categories, sidebar_rooms, sidebar_peers) =
        super::load_sidebar(state, user, None).await?;
    let fragment = SidebarUpdateFragment {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
    };
    html(&fragment)
}
