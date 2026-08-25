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
    DescriptionEditFragment, DescriptionViewFragment, FileListFragment, RoomFileRow, RoomInfoPage,
    RoomNicknameFragment, WikiEditFragment, WikiViewFragment,
};
use crate::views::{html, Html};

/// LC-87: page size for the Files tab. Big enough to fill the visible
/// area on most screens; small enough that the initial render is cheap
/// even on rooms with tens of thousands of uploads.
const FILES_PAGE_SIZE: i64 = 40;

#[derive(Deserialize)]
pub struct InfoQuery {
    /// `"docs"` (default) or `"pinned"` or `"files"`. Anything else falls
    /// back to `"docs"`.
    #[serde(default)]
    pub tab: Option<String>,
    /// LC-87: active file-kind filter on the Files tab. Anything we do
    /// not recognise falls back to `"all"`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Deserialize)]
pub struct FilesQuery {
    #[serde(default)]
    pub kind: Option<String>,
    /// Cursor for pagination. Each row's `id` is monotonically
    /// increasing; the load-more button sends the smallest id on the
    /// current page as `before`.
    #[serde(default)]
    pub before: Option<i64>,
    /// True when the fragment should append (load-more) rather than
    /// replace (filter change). Drives `hx-swap-oob` / wrapper
    /// rendering in the template.
    #[serde(default)]
    pub append: Option<bool>,
}

fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/")
}

fn kind_icon_for(mime: &str) -> &'static str {
    if mime.starts_with("video/") {
        "V"
    } else if mime.starts_with("audio/") {
        "A"
    } else if mime == "application/pdf" {
        "P"
    } else {
        "F"
    }
}

/// Materialise file rows + the next-page cursor in one query plus a
/// bulk auth-label lookup. Peeks one extra row past `limit` to decide
/// whether the load-more button should render.
async fn load_room_files(
    state: &AppState,
    room_id: i64,
    kind: db::uploads::FileKindFilter,
    before_id: Option<i64>,
) -> Result<(Vec<RoomFileRow>, Option<i64>), AppError> {
    let mut rows =
        db::uploads::list_room_files(&state.chat, room_id, kind, before_id, FILES_PAGE_SIZE + 1)
            .await?;
    let next_cursor = if rows.len() as i64 > FILES_PAGE_SIZE {
        rows.truncate(FILES_PAGE_SIZE as usize);
        rows.last().map(|r| r.id)
    } else {
        None
    };
    let ids: Vec<&str> = rows.iter().map(|r| r.uploader_id.as_str()).collect();
    let labels: std::collections::HashMap<String, (String, Option<String>)> =
        db::auth::display_names_for_ids(&state.auth, &ids)
            .await?
            .into_iter()
            .collect();
    let view_rows: Vec<RoomFileRow> = rows
        .into_iter()
        .map(|r| {
            let label = labels
                .get(&r.uploader_id)
                .map(|(uname, dname)| label_for(uname, dname.as_deref()))
                .unwrap_or_else(|| format!("@{}", r.uploader_id));
            let mid = r.message_id.unwrap_or(0);
            RoomFileRow {
                id: r.id,
                message_id: mid,
                filename: r.filename,
                mime_type: r.mime_type.clone(),
                size_bytes: r.size_bytes,
                created_at: r.created_at,
                uploader_label: label,
                is_image: is_image_mime(&r.mime_type),
                kind_icon: kind_icon_for(&r.mime_type),
            }
        })
        .collect();
    Ok((view_rows, next_cursor))
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
    Query(InfoQuery { tab, kind }): Query<InfoQuery>,
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
        Some("files") => "files",
        _ => "docs",
    };

    let files_kind_raw = kind.as_deref().unwrap_or("all");
    let files_kind_filter = db::uploads::FileKindFilter::from_query(files_kind_raw);
    let files_kind = files_kind_filter.as_query();

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
    let enclave_role = if let Some(eid) = current_enclave {
        db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role)
    } else {
        None
    };
    let can_manage_overrides = crate::perms::room_can_manage_overrides(enclave_role, &user.role);
    // LC-400: enclave owner/admin (or site admin) can delete the room from this
    // page. The delete-room affordance was orphaned when LC-336 removed the
    // enclave landing menu; this restores it on the room's own info page, gated
    // by the same `enclave_can_manage` that `post_delete_room` enforces.
    let can_manage = crate::perms::enclave_can_manage(enclave_role, &user.role);

    // LC-449: the redesigned info page (settings-shell, client-side tabs) keeps
    // all three panels in the DOM, so load pinned + files unconditionally rather
    // than gating on `active_tab` (which now only sets the INITIAL panel). Both
    // are bounded scans (pinned <= MAX_PINS_PER_ROOM, files = one page), cheap
    // enough to render on every info-page load.
    let pinned: Vec<PinnedListRow> = {
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
    };

    let (files, files_next_cursor) =
        load_room_files(&state, room_id, files_kind_filter, None).await?;

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

    // LC-321: the viewer's own nickname in this room, pre-filling the docs-tab
    // form.
    let nick_value = db::room_nicknames::get(&state.chat, room_id, &user.id).await?;

    html(&RoomInfoPage {
        user: &user,
        room: &room,
        active_tab,
        wiki_html,
        description_html,
        wiki_updated_by_label,
        can_edit_wiki,
        can_manage_overrides,
        can_manage,
        enclave_id: current_enclave,
        pinned: &pinned,
        files: &files,
        files_kind,
        files_next_cursor,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        nick_room_id: room_id,
        nick_value,
        nick_room_name: room.name.clone(),
    })
}

/// POST /room/{id}/nickname
///
/// LC-321: set or clear the caller's own per-room nickname. Self-only (the
/// nickname is always the authed user's). Access-gated like the info page. An
/// empty / absent `nickname` field clears it. Returns the re-rendered
/// `#room-nickname` section for an in-place HTMX swap.
#[derive(serde::Deserialize)]
pub struct NicknameForm {
    #[serde(default)]
    nickname: String,
}

pub async fn post_nickname(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<NicknameForm>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let trimmed = form.nickname.trim();
    if trimmed.is_empty() {
        db::room_nicknames::clear(&state.chat, room_id, &user.id).await?;
    } else {
        db::room_nicknames::set(&state.chat, room_id, &user.id, trimmed)
            .await
            .map_err(|e| match e {
                db::room_nicknames::SetNicknameError::TooLong(n) => {
                    AppError::BadRequest(format!("nickname exceeds {n} characters"))
                }
                db::room_nicknames::SetNicknameError::Db(e) => AppError::from(e),
            })?;
    }
    let nick_value = db::room_nicknames::get(&state.chat, room_id, &user.id).await?;
    // LC-809: the swapped-in section keeps naming its room, so re-resolve the
    // room name (the info page passes it from the loaded room; this fragment
    // path did not load one).
    let nick_room_name = db::chat::get_room(&state.chat, room_id)
        .await?
        .map(|r| r.name)
        .unwrap_or_default();
    // LC-454: swap the #room-nickname section in place AND append a toast so the
    // save is never silent (mirrors the settings save-feedback pattern). The
    // section partial stays toast-free so a fresh page load does not emit one.
    let saved = nick_value.is_some();
    let frag = html(&RoomNicknameFragment {
        nick_room_id: room_id,
        nick_value,
        nick_room_name,
    })?;
    let msg = crate::i18n::translate_current(if saved {
        "room-nickname-saved"
    } else {
        "room-nickname-cleared"
    });
    let toast = html(&crate::views::settings::SettingsFeedback::toast_only_ok(
        msg,
    ))?;
    Ok(Html(format!("{}{}", frag.0, toast.0)))
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

/// LC-153-WIKI-DESC-CAP (#283): room description and wiki bodies render
/// through the same markdown + LaTeX pipeline as chat messages, but had no
/// length bound - the only limit was Axum's ~2 MiB `DefaultBodyLimit`, so a
/// privileged-but-untrusted room moderator could feed a multi-MiB body to the
/// renderer on every save (and to every viewer on every page load). Cap them
/// the way chat messages are capped (LC-153, `room::check_message_length`),
/// with a doc-appropriate bound: generous for a wiki page, ~32x below the
/// framework body limit, and small enough to keep render + storage + broadcast
/// cheap. `markdown::render` is synchronous and called inline everywhere (the
/// chat post path included; only `warm_syntect` is offloaded), so bounding the
/// INPUT - not offloading the render - is the actual mitigation.
const MAX_DOC_CHARS: usize = 64_000;

/// Reject an over-length description/wiki body before any DB write or render.
/// `body` is the already-trimmed text; counts characters (not bytes) so
/// multibyte (CJK / emoji) docs are not unfairly penalized, matching
/// `room::check_message_length`.
fn check_doc_length(body: &str) -> Result<(), AppError> {
    if body.chars().count() > MAX_DOC_CHARS {
        return Err(AppError::BadRequest(format!(
            "document too long (max {MAX_DOC_CHARS} characters)"
        )));
    }
    Ok(())
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
    check_doc_length(trimmed)?;
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
    check_doc_length(trimmed)?;
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

/// GET /room/{id}/files
///
/// HTMX fragment for the Files tab. Two callers:
/// 1. Filter change (`?kind=image&append=false`) → replace `#files-grid`.
/// 2. Load-more (`?before=N&kind=...&append=true`) → append rows.
///
/// Access is gated the same way as the page handler. Soft-deleted
/// parent messages are filtered out by the DB layer.
pub async fn get_files(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Query(FilesQuery {
        kind,
        before,
        append,
    }): Query<FilesQuery>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let kind_raw = kind.as_deref().unwrap_or("all");
    let kind_filter = db::uploads::FileKindFilter::from_query(kind_raw);
    let (rows, next_cursor) = load_room_files(&state, room_id, kind_filter, before).await?;
    html(&FileListFragment {
        room_id,
        files: &rows,
        kind: kind_filter.as_query(),
        next_cursor,
        append: append.unwrap_or(false),
    })
}
