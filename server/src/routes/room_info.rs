//! LC-86: room info page + wiki edit endpoints.
//!
//! `/room/{id}/info` is a full-page view with a tab nav. The default
//! tab ("docs") shows the room's long-form description and a single
//! wiki page, both rendered through `views::markdown`. The other tab
//! ("pinned") reuses the existing pinned-message list.
//!
//! Wiki editing is HTMX-inline: `GET /room/{id}/wiki/edit` swaps in a
//! textarea, `PATCH /room/{id}/wiki` writes the new body and returns
//! the re-rendered view fragment. Both are gated on the viewer's
//! effective Moderator role for the room (LC-84).
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::pinned::PinnedListRow;
use crate::views::room_info::{
    DescriptionEditFragment, DescriptionViewFragment, RoomInfoPage, WikiEditFragment,
    WikiViewFragment,
};
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct InfoQuery {
    /// `"docs"` (default) or `"pinned"`. Anything else falls back to `"docs"`.
    #[serde(default)]
    pub tab: Option<String>,
}

fn label_for(username: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => format!("@{username}"),
    }
}

/// Render the markdown body to HTML through the shared pipeline. Wiki
/// and description bodies do not carry per-message mention rows the
/// way chat messages do, so the mention slice is empty; `@user` would
/// render as literal text. Custom emoji refs come from the room's
/// enclave so `:shortcode:` resolves the same way it does in chat.
async fn render_markdown_body(
    state: &AppState,
    room_id: i64,
    body: &str,
) -> Result<String, AppError> {
    let emojis = db::custom_emojis::refs_for_room(&state.chat, room_id).await?;
    Ok(crate::views::markdown::render(body, &[], &emojis))
}

/// GET /room/{id}/info
pub async fn get_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Query(InfoQuery { tab }): Query<InfoQuery>,
) -> Result<Html, AppError> {
    // Same access predicate as the room page: any room member, or site
    // admin, or any user for `public` rooms within accessible enclaves.
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let active_tab = match tab.as_deref() {
        Some("pinned") => "pinned",
        _ => "docs",
    };

    // Render markdown for description / wiki up front so the template
    // stays presentation-only.
    let description_html = if let Some(d) = room.description.as_deref().filter(|s| !s.is_empty()) {
        render_markdown_body(&state, room_id, d).await?
    } else {
        String::new()
    };
    let wiki_html = if let Some(b) = room.wiki_body.as_deref().filter(|s| !s.is_empty()) {
        render_markdown_body(&state, room_id, b).await?
    } else {
        String::new()
    };
    let wiki_updated_by_label = match room.wiki_updated_by.as_deref() {
        Some(uid) => db::auth::find_user_by_id(&state.auth, uid)
            .await?
            .map(|r| label_for(&r.username, r.display_name.as_deref())),
        None => None,
    };

    // Mod+ can edit the wiki; the admin-tier subset (enclave_can_manage)
    // can edit the description / topic via the existing enclave-rooms
    // edit form. The description-edit affordance on THIS page is a link
    // back to that form.
    let can_edit_wiki =
        db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await?;
    let current_enclave = super::enclave_for_room(&state, room_id).await?;
    let can_manage_overrides = {
        let er = if let Some(eid) = current_enclave {
            db::enclave::get_membership(&state.chat, eid, &user.id)
                .await?
                .map(|m| m.role)
        } else {
            None
        };
        crate::perms::room_can_manage_overrides(er, &user.role)
    };

    // Pinned rows only when the pinned tab is active. Cheap to skip
    // otherwise.
    let pinned: Vec<PinnedListRow> = if active_tab == "pinned" {
        let pins =
            db::pinned::pins_for_room(&state.chat, room_id, db::pinned::MAX_PINS_PER_ROOM).await?;
        let names = super::pinned::resolve_author_labels(&state, &pins).await?;
        pins.iter()
            .map(|p| PinnedListRow {
                message_id: p.message_id,
                author_label: names
                    .get(&p.author_user_id)
                    .cloned()
                    .unwrap_or_else(|| p.author_user_id.clone()),
                pinner_label: names
                    .get(&p.pinned_by_user_id)
                    .cloned()
                    .unwrap_or_else(|| p.pinned_by_user_id.clone()),
                pinned_at: p.pinned_at.clone(),
                body: p.body.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, current_enclave).await?;

    html(&RoomInfoPage {
        user: &user,
        room: &room,
        active_tab,
        wiki_html,
        description_html,
        wiki_updated_by_label,
        can_edit_wiki,
        can_manage_overrides,
        pinned: &pinned,
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

async fn require_can_edit_wiki(
    state: &AppState,
    user: &User,
    room_id: i64,
) -> Result<(), AppError> {
    let ok = db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await?;
    if !ok {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// GET /room/{id}/wiki
///
/// View-only wiki fragment for HTMX cancel-out-of-edit. Same shape the
/// `PATCH /room/{id}/wiki` handler returns on save, so swapping into
/// `#wiki-content` from either path renders the same DOM.
pub async fn get_wiki(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let html_body = if let Some(b) = room.wiki_body.as_deref().filter(|s| !s.is_empty()) {
        render_markdown_body(&state, room_id, b).await?
    } else {
        String::new()
    };
    let updated_by_label = match room.wiki_updated_by.as_deref() {
        Some(uid) => db::auth::find_user_by_id(&state.auth, uid)
            .await?
            .map(|r| label_for(&r.username, r.display_name.as_deref())),
        None => None,
    };
    let can_edit_wiki =
        db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await?;
    html(&WikiViewFragment {
        room_id,
        html: html_body,
        updated_at: room.wiki_updated_at.as_deref(),
        updated_by_label: updated_by_label.as_deref(),
        can_edit_wiki,
    })
}

async fn require_can_edit_description(
    state: &AppState,
    user: &User,
    room_id: i64,
) -> Result<(), AppError> {
    let er = if let Some(eid) = super::enclave_for_room(state, room_id).await? {
        db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role)
    } else {
        None
    };
    if !crate::perms::room_can_manage_overrides(er, &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// GET /room/{id}/description
///
/// View-only description fragment for HTMX cancel-out-of-edit. Anyone
/// who can read the room can see it.
pub async fn get_description(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let html_body = if let Some(d) = room.description.as_deref().filter(|s| !s.is_empty()) {
        render_markdown_body(&state, room_id, d).await?
    } else {
        String::new()
    };
    let can_edit_description = require_can_edit_description(&state, &user, room_id)
        .await
        .is_ok();
    html(&DescriptionViewFragment {
        room_id,
        html: html_body,
        can_edit_description,
    })
}

/// GET /room/{id}/description/edit
pub async fn get_description_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_can_edit_description(&state, &user, room_id).await?;
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    html(&DescriptionEditFragment {
        room_id,
        body: room.description.as_deref().unwrap_or(""),
    })
}

#[derive(Deserialize)]
pub struct DescriptionForm {
    pub body: String,
}

/// PATCH /room/{id}/description
pub async fn patch_description(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<DescriptionForm>,
) -> Result<impl IntoResponse, AppError> {
    require_can_edit_description(&state, &user, room_id).await?;
    let trimmed = form.body.trim();
    let body_opt = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    let n = db::chat::set_room_description(&state.chat, room_id, body_opt).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let html_body = if let Some(d) = room.description.as_deref().filter(|s| !s.is_empty()) {
        render_markdown_body(&state, room_id, d).await?
    } else {
        String::new()
    };
    html(&DescriptionViewFragment {
        room_id,
        html: html_body,
        can_edit_description: true,
    })
}

/// GET /room/{id}/wiki/edit
///
/// Returns the inline textarea editor for the wiki body. Mod+ only.
pub async fn get_wiki_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_can_edit_wiki(&state, &user, room_id).await?;
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    html(&WikiEditFragment {
        room_id,
        body: room.wiki_body.as_deref().unwrap_or(""),
    })
}

#[derive(Deserialize)]
pub struct WikiForm {
    pub body: String,
}

/// PATCH /room/{id}/wiki
///
/// Persist a new wiki body, stamp the last-edit metadata, log a
/// `mod_actions` audit row, and return the re-rendered view fragment.
pub async fn patch_wiki(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<WikiForm>,
) -> Result<impl IntoResponse, AppError> {
    require_can_edit_wiki(&state, &user, room_id).await?;
    let trimmed = form.body.trim();
    let body_opt = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    let n = db::chat::set_room_wiki(&state.chat, room_id, body_opt, &user.id).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    db::moderation::log_mod_action(
        &state.chat,
        "room_wiki_edit",
        "",
        &user.id,
        None,
        Some(room_id),
        None,
    )
    .await?;

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let html_body = if let Some(b) = room.wiki_body.as_deref().filter(|s| !s.is_empty()) {
        render_markdown_body(&state, room_id, b).await?
    } else {
        String::new()
    };
    let updated_by_label = match room.wiki_updated_by.as_deref() {
        Some(uid) => db::auth::find_user_by_id(&state.auth, uid)
            .await?
            .map(|r| label_for(&r.username, r.display_name.as_deref())),
        None => None,
    };
    let frag = WikiViewFragment {
        room_id,
        html: html_body,
        updated_at: room.wiki_updated_at.as_deref(),
        updated_by_label: updated_by_label.as_deref(),
        can_edit_wiki: true,
    };
    html(&frag)
}
