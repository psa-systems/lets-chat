use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct FlashQuery {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Resolve a known error code (with optional caller-supplied name) into the
/// user-facing message that the template renders. Unknown codes drop to
/// `None` so an untrusted query param cannot push attacker text onto the
/// page. The `name` field is rendered through Askama's default
/// HTML-escaping, and is also length-clamped here so a megabyte of "name="
/// cannot blow out the banner.
fn flash_message(code: Option<&str>, name: Option<&str>) -> Option<String> {
    let trimmed = name
        .map(|n| n.chars().take(64).collect::<String>())
        .filter(|n| !n.is_empty());
    match code? {
        "enclave_name_taken" => Some(match trimmed {
            Some(n) => format!("Enclave \"{n}\" already exists. Pick a different name."),
            None => "That enclave name is already taken. Pick a different name.".to_string(),
        }),
        "room_name_taken" => Some(match trimmed {
            Some(n) => format!("Room \"{n}\" already exists. Pick a different name."),
            None => "That room name is already taken. Pick a different name.".to_string(),
        }),
        _ => None,
    }
}

fn flash_name_param(name: &str) -> String {
    percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC).to_string()
}

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::enclave::{EnclaveMembership, EnclaveRole};
use crate::models::User;
use crate::perms::enclave_can_manage;
use crate::perms::{enclave_can_delete, enclave_can_manage_admins};
use crate::state::AppState;
use crate::views::enclave::{
    DiscoverPage, EnclaveInviteCandidate, EnclaveInviteCandidateState, EnclaveInviteRowResult,
    EnclaveInviteSearchFragment, EnclaveMemberView, EnclavePage, EnclaveSettingsPage,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// Resolve each membership row to an `EnclaveMemberView` carrying a
/// human-readable label (display_name when set, otherwise `@username`,
/// otherwise the raw user_id as a last resort).
async fn resolve_member_views(
    state: &AppState,
    members: Vec<EnclaveMembership>,
) -> Result<Vec<EnclaveMemberView>, AppError> {
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let label = match db::auth::find_user_by_id(&state.auth, &m.user_id).await? {
            Some(rec) => match rec.display_name.as_deref() {
                Some(n) if !n.trim().is_empty() => n.to_string(),
                _ => format!("@{}", rec.username),
            },
            None => m.user_id.clone(),
        };
        out.push(EnclaveMemberView {
            user_id: m.user_id,
            label,
            role: m.role,
        });
    }
    Ok(out)
}

async fn broadcast_to_enclave(state: &AppState, enclave_id: i64, event: &ChatEvent) {
    if let Ok(members) = db::enclave::list_members(&state.chat, enclave_id).await {
        for m in members {
            state.hub.broadcast_to_user(&m.user_id, event);
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enclaves", post(post_create))
        .route("/enclave/{id}", get(get_landing))
        .route("/enclaves/discover", get(get_discover))
        .route("/enclaves/discover/{id}/join", post(post_discover_join))
        .route("/enclaves/join", post(post_join_by_code))
        .route("/enclave/{id}/visibility", post(post_visibility))
        .route("/enclave/{id}/share-emojis", post(post_share_emojis))
        .route(
            "/enclave/{id}/invite-code",
            post(post_invite_code).delete(delete_invite_code),
        )
        .route("/enclave/{id}/invite", post(post_invite))
        .route("/enclave/{id}/invite/search", get(get_invite_search))
        .route("/invitations", get(get_invitations))
        .route("/invitations/{id}/accept", post(post_invitation_accept))
        .route("/invitations/{id}/decline", post(post_invitation_decline))
        .route("/enclave/{id}/settings", get(get_settings))
        .route("/enclave/{id}/edit", post(post_edit))
        .route("/enclave/{id}/transfer", post(post_transfer))
        .route("/enclave/{id}/delete", post(post_delete))
        .route("/enclave/{id}/leave", post(post_leave))
        .route(
            "/enclave/{id}/members/{user_id}/role",
            post(post_member_role),
        )
        .route("/enclave/{id}/members/{user_id}/kick", post(post_kick))
        .route("/enclave/{id}/rooms", post(post_create_room))
        .route("/enclave/{id}/rooms/{room_id}/edit", post(post_edit_room))
        .route(
            "/enclave/{id}/rooms/{room_id}/delete",
            post(post_delete_room),
        )
        .route(
            "/enclave/{id}/rooms/{room_id}/members",
            post(post_add_room_member),
        )
        .route(
            "/enclave/{id}/rooms/{room_id}/members/{user_id}/remove",
            post(post_remove_room_member),
        )
        // Custom emoji upload streams its own multipart body; disable Axum's
        // default 2 MiB cap on this route so the 256 KiB handler limit is
        // the only ceiling.
        .route(
            "/enclave/{id}/emojis",
            post(super::custom_emojis::post_upload).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/enclave/{id}/emojis/{emoji_id}/delete",
            post(super::custom_emojis::post_delete),
        )
}

async fn require_manage(state: &AppState, user: &User, enclave_id: i64) -> Result<(), AppError> {
    let m = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
    if !enclave_can_manage(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(d) if d.is_unique_violation())
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub name: String,
    pub description: Option<String>,
}

pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<impl IntoResponse, AppError> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let id = match db::enclave::create_enclave(
        &state.chat,
        name,
        form.description.as_deref().filter(|s| !s.is_empty()),
        &user.id,
    )
    .await
    {
        Ok(id) => id,
        Err(e) if is_unique_violation(&e) => {
            return Ok(Redirect::to(&format!(
                "/enclaves/discover?error=enclave_name_taken&name={}",
                flash_name_param(name)
            )));
        }
        Err(e) => return Err(e.into()),
    };
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn get_landing(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(flash): Query<FlashQuery>,
) -> Result<Html, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    let membership = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    let role = membership.as_ref().map(|m| m.role);
    let is_site_admin = user.role == "admin";
    if role.is_none() && !is_site_admin {
        return Err(AppError::Forbidden);
    }
    let can_manage = enclave_can_manage(role, &user.role);
    let members = db::enclave::list_members(&state.chat, id).await?;
    let member_views = resolve_member_views(&state, members).await?;
    let rooms = db::chat::list_rooms_in_enclave(&state.chat, id, &user.id, can_manage).await?;
    let (
        sidebar_categories,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, Some(id)).await?;
    html(&EnclavePage {
        user: &user,
        enclave: &enclave,
        members: &member_views,
        rooms: &rooms,
        can_manage,
        flash_error: flash_message(flash.error.as_deref(), flash.name.as_deref()).as_deref(),
        sidebar_categories: &sidebar_categories,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}

pub async fn get_discover(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(flash): Query<FlashQuery>,
) -> Result<Html, AppError> {
    let enclaves = db::enclave::list_public_enclaves(&state.chat).await?;
    let (
        sidebar_categories,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    html(&DiscoverPage {
        user: &user,
        enclaves: &enclaves,
        flash_error: flash_message(flash.error.as_deref(), flash.name.as_deref()).as_deref(),
        sidebar_categories: &sidebar_categories,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}

#[derive(Deserialize)]
pub struct VisibilityForm {
    pub is_public: String,
}

pub async fn post_visibility(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<VisibilityForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    db::enclave::set_public(&state.chat, id, form.is_public == "1").await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

#[derive(Deserialize)]
pub struct ShareEmojisForm {
    pub share: String,
}

/// Toggle whether the enclave's custom emojis resolve outside its own
/// rooms. Gated on `enclave_can_manage`. Existing membership requirements
/// for posting and joining are unaffected; only emoji lookup changes.
pub async fn post_share_emojis(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<ShareEmojisForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    db::enclave::set_share_emojis_globally(&state.chat, id, form.share == "1").await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_invite_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    db::enclave::regenerate_invite_code(&state.chat, id, &code).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn delete_invite_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    db::enclave::clear_invite_code(&state.chat, id).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_discover_join(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    if !enclave.is_public {
        return Err(AppError::Forbidden);
    }
    if db::enclave::get_membership(&state.chat, id, &user.id)
        .await?
        .is_some()
    {
        return Ok(Redirect::to(&format!("/enclave/{id}")));
    }
    db::enclave::add_member(&state.chat, id, &user.id, EnclaveRole::Member).await?;
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::EnclaveMemberAdded {
            enclave_id: id,
            user_id: user.id.clone(),
        },
    );
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

#[derive(Deserialize)]
pub struct JoinByCodeForm {
    pub code: String,
}

pub async fn post_join_by_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<JoinByCodeForm>,
) -> Result<impl IntoResponse, AppError> {
    let Some(enclave) =
        db::enclave::get_enclave_by_invite_code(&state.chat, form.code.trim()).await?
    else {
        return Err(AppError::BadRequest("invalid or revoked code".into()));
    };
    if db::enclave::get_membership(&state.chat, enclave.id, &user.id)
        .await?
        .is_some()
    {
        return Ok(Redirect::to(&format!("/enclave/{}", enclave.id)));
    }
    db::enclave::add_member(&state.chat, enclave.id, &user.id, EnclaveRole::Member).await?;
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::EnclaveMemberAdded {
            enclave_id: enclave.id,
            user_id: user.id.clone(),
        },
    );
    Ok(Redirect::to(&format!("/enclave/{}", enclave.id)))
}

#[derive(Deserialize)]
pub struct InviteForm {
    pub user_id: String,
}

#[derive(Deserialize)]
pub struct InviteSearchQuery {
    #[serde(default)]
    pub q: Option<String>,
}

/// GET /enclave/{id}/invite/search?q=... - typeahead candidates for inviting
/// a new member. Mirrors `/users/search`: blank query returns an empty body
/// so the popover collapses via `empty:hidden`. Each row carries enough
/// state for the template to render an Invite button, a "Member" pill, or
/// an "Invited" pill, all without per-row queries.
pub async fn get_invite_search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(InviteSearchQuery { q }): Query<InviteSearchQuery>,
) -> Result<Html, AppError> {
    require_manage(&state, &user, id).await?;
    let query = q.unwrap_or_default();
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Html(String::new()));
    }

    let records = db::auth::search_users(&state.auth, trimmed, &user.id, 50).await?;
    let members = db::enclave::list_members(&state.chat, id).await?;
    let member_ids: std::collections::HashSet<String> =
        members.into_iter().map(|m| m.user_id).collect();
    let invited_ids: std::collections::HashSet<String> =
        db::enclave::pending_invitee_ids_for_enclave(&state.chat, id)
            .await?
            .into_iter()
            .collect();

    let results: Vec<EnclaveInviteCandidate> = records
        .into_iter()
        .map(|r| {
            let state_flag = if r.id == user.id {
                EnclaveInviteCandidateState::Self_
            } else if member_ids.contains(&r.id) {
                EnclaveInviteCandidateState::AlreadyMember
            } else if invited_ids.contains(&r.id) {
                EnclaveInviteCandidateState::AlreadyInvited
            } else {
                EnclaveInviteCandidateState::Invitable
            };
            EnclaveInviteCandidate {
                id: r.id,
                username: r.username,
                display_name: r.display_name,
                avatar_ext: r.avatar_ext,
                status: r.status,
                custom_status: r.custom_status,
                state: state_flag,
            }
        })
        .collect();

    html(&EnclaveInviteSearchFragment {
        enclave_id: id,
        query: trimmed,
        results: &results,
    })
}

pub async fn post_invite(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<InviteForm>,
) -> Result<Html, AppError> {
    require_manage(&state, &user, id).await?;
    let target_id = form.user_id.trim();
    if target_id.is_empty() {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "Pick a person.",
        });
    }
    let Some(target) = db::auth::find_user_by_id(&state.auth, target_id).await? else {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "User not found.",
        });
    };
    if target.id == user.id {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "You can't invite yourself.",
        });
    }
    // Mirror the discovery filters that the typeahead applies, so a hand-
    // crafted POST cannot bypass them. Without this gate a manager could
    // construct a request to invite a user who set their profile private,
    // a user they have a mutual block with, or a banned account - none of
    // which would have appeared in the typeahead in the first place. The
    // returned message is deliberately generic for the private/block cases
    // so the response cannot be used to probe block state or to enumerate
    // users with private profiles.
    if target.is_banned {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "User not found.",
        });
    }
    if !target.is_profile_public {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "User not found.",
        });
    }
    if db::auth::is_blocked_either_way(&state.auth, &user.id, &target.id).await? {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "User not found.",
        });
    }
    if db::enclave::get_membership(&state.chat, id, &target.id)
        .await?
        .is_some()
    {
        return html(&EnclaveInviteRowResult {
            ok: false,
            message: "Already a member.",
        });
    }
    if let Err(e) = db::enclave::create_invitation(&state.chat, id, &target.id, &user.id).await {
        // A second click on the same Invite button (or a concurrent request)
        // races on `(enclave_id, invitee_id)`. Surface it as the friendly
        // "already invited" state rather than a 500.
        if matches!(&e, sqlx::Error::Database(d) if d.is_unique_violation()) {
            return html(&EnclaveInviteRowResult {
                ok: true,
                message: "Invited",
            });
        }
        return Err(e.into());
    }
    state.hub.broadcast_to_user(
        &target.id,
        &ChatEvent::EnclaveInvitationCreated {
            invitee_id: target.id.clone(),
        },
    );
    html(&EnclaveInviteRowResult {
        ok: true,
        message: "Invited",
    })
}

pub async fn post_invitation_accept(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(inv) = db::enclave::get_invitation(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    if inv.invitee_id != user.id {
        return Err(AppError::Forbidden);
    }
    let (eid, _) = db::enclave::accept_invitation(&state.chat, id).await?;
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::EnclaveMemberAdded {
            enclave_id: eid,
            user_id: user.id.clone(),
        },
    );
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::EnclaveInvitationResolved {
            invitee_id: user.id.clone(),
        },
    );
    Ok(Redirect::to(&format!("/enclave/{eid}")))
}

pub async fn post_invitation_decline(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(inv) = db::enclave::get_invitation(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    if inv.invitee_id != user.id {
        return Err(AppError::Forbidden);
    }
    db::enclave::delete_invitation(&state.chat, id).await?;
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::EnclaveInvitationResolved {
            invitee_id: user.id.clone(),
        },
    );
    Ok(Redirect::to("/invitations"))
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(flash): Query<FlashQuery>,
) -> Result<Html, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    let role = m.as_ref().map(|x| x.role);
    if !enclave_can_manage(role, &user.role) {
        return Err(AppError::Forbidden);
    }
    let can_delete = enclave_can_delete(role, &user.role);
    let members = db::enclave::list_members(&state.chat, id).await?;
    let member_views = resolve_member_views(&state, members).await?;
    let emojis = db::custom_emojis::list_for_enclave(&state.chat, id).await?;
    let (
        sidebar_categories,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, Some(id)).await?;
    html(&EnclaveSettingsPage {
        user: &user,
        enclave: &enclave,
        members: &member_views,
        emojis: &emojis,
        can_delete,
        flash_error: flash_message(flash.error.as_deref(), flash.name.as_deref()).as_deref(),
        sidebar_categories: &sidebar_categories,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}

#[derive(Deserialize)]
pub struct EditForm {
    pub name: String,
    pub description: Option<String>,
}

pub async fn post_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<EditForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    if let Err(e) = db::enclave::update_metadata(
        &state.chat,
        id,
        name,
        form.description.as_deref().filter(|s| !s.is_empty()),
    )
    .await
    {
        if is_unique_violation(&e) {
            return Ok(Redirect::to(&format!(
                "/enclave/{id}/settings?error=enclave_name_taken&name={}",
                flash_name_param(name)
            )));
        }
        return Err(e.into());
    }
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

#[derive(Deserialize)]
pub struct TransferForm {
    pub new_owner_id: String,
}

pub async fn post_transfer(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<TransferForm>,
) -> Result<impl IntoResponse, AppError> {
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    if !enclave_can_manage_admins(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    db::enclave::transfer_ownership(&state.chat, id, form.new_owner_id.trim()).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    if !enclave_can_delete(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    let former_members = db::enclave::list_members(&state.chat, id).await?;
    db::enclave::delete_enclave(&state.chat, id).await?;
    for fm in former_members {
        state.hub.broadcast_to_user(
            &fm.user_id,
            &ChatEvent::EnclaveMemberRemoved {
                enclave_id: id,
                user_id: fm.user_id.clone(),
            },
        );
    }
    Ok(Redirect::to("/"))
}

pub async fn post_leave(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(m) = db::enclave::get_membership(&state.chat, id, &user.id).await? else {
        return Err(AppError::NotFound);
    };
    if matches!(m.role, EnclaveRole::Owner) {
        let members = db::enclave::list_members(&state.chat, id).await?;
        if members.len() == 1 {
            return Err(AppError::BadRequest(
                "delete the enclave instead of leaving".into(),
            ));
        }
        return Err(AppError::BadRequest(
            "transfer ownership before leaving".into(),
        ));
    }
    // LC-79 redesign: categorization is per-enclave / shared. Leaving
    // an enclave doesn't dirty any user-specific assignment row (there
    // aren't any). The assignment rows live with the enclave and stay
    // valid for the remaining members.
    db::enclave::remove_member(&state.chat, id, &user.id).await?;
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::EnclaveMemberRemoved {
            enclave_id: id,
            user_id: user.id.clone(),
        },
    );
    Ok(Redirect::to("/"))
}

#[derive(Deserialize)]
pub struct RoleForm {
    pub role: String,
}

pub async fn post_member_role(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, target)): Path<(i64, String)>,
    axum::Form(form): axum::Form<RoleForm>,
) -> Result<impl IntoResponse, AppError> {
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    if !enclave_can_manage_admins(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    let new_role = match form.role.as_str() {
        "admin" => EnclaveRole::Admin,
        "member" => EnclaveRole::Member,
        _ => return Err(AppError::BadRequest("invalid role".into())),
    };
    db::enclave::update_role(&state.chat, id, &target, new_role).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_kick(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, target)): Path<(i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let Some(target_m) = db::enclave::get_membership(&state.chat, id, &target).await? else {
        return Err(AppError::NotFound);
    };
    if matches!(target_m.role, EnclaveRole::Owner) {
        return Err(AppError::BadRequest(
            "cannot kick the owner; transfer ownership first".into(),
        ));
    }
    // LC-79 redesign: per-enclave categorization is shared, not
    // per-user. Kicking a user does not affect any assignment row.
    db::enclave::remove_member(&state.chat, id, &target).await?;
    state.hub.broadcast_to_user(
        &target,
        &ChatEvent::EnclaveMemberRemoved {
            enclave_id: id,
            user_id: target.clone(),
        },
    );
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

#[derive(Deserialize)]
pub struct RoomForm {
    pub name: String,
    pub topic: Option<String>,
    /// Visibility: "public" | "private".
    pub room_type: String,
    /// Channel kind: "text" | "voice". Defaults to text for older form
    /// posts that predate the voice/text split.
    #[serde(default)]
    pub kind: Option<String>,
}

pub async fn post_create_room(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<RoomForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    // Visibility and channel kind are independent axes: a channel is public
    // or private (`room_type`), and separately text or voice (`kind`).
    if !matches!(form.room_type.as_str(), "public" | "private") {
        return Err(AppError::BadRequest("invalid room_type".into()));
    }
    let is_voice = match form.kind.as_deref() {
        None | Some("text") => false,
        Some("voice") => true,
        Some(_) => return Err(AppError::BadRequest("invalid kind".into())),
    };
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let invite_code = if form.room_type == "private" {
        let c: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(10)
            .map(char::from)
            .collect();
        Some(c)
    } else {
        None
    };
    let topic = form.topic.as_deref().filter(|s| !s.is_empty());
    let created = if is_voice {
        db::chat::create_voice_room(
            &state.chat,
            name,
            topic,
            &form.room_type,
            invite_code.as_deref(),
            Some(id),
        )
        .await
    } else {
        db::chat::create_room(
            &state.chat,
            name,
            topic,
            &form.room_type,
            invite_code.as_deref(),
            Some(id),
        )
        .await
    };
    let room_id = match created {
        Ok(rid) => rid,
        Err(e) if is_unique_violation(&e) => {
            return Ok(Redirect::to(&format!(
                "/enclave/{id}?error=room_name_taken&name={}",
                flash_name_param(name)
            )));
        }
        Err(e) => return Err(e.into()),
    };
    if form.room_type == "private" {
        db::chat::add_room_member(&state.chat, room_id, &user.id).await?;
    }
    broadcast_to_enclave(
        &state,
        id,
        &ChatEvent::EnclaveRoomAdded {
            enclave_id: id,
            room_id,
        },
    )
    .await;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

#[derive(Deserialize)]
pub struct RoomEditForm {
    pub name: String,
    pub topic: Option<String>,
}

async fn assert_room_in_enclave(
    pool: &sqlx::SqlitePool,
    enclave_id: i64,
    room_id: i64,
) -> Result<(), AppError> {
    let row = sqlx::query("SELECT enclave_id FROM rooms WHERE id=?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    let Some(r) = row else {
        return Err(AppError::NotFound);
    };
    let eid: Option<i64> = sqlx::Row::get(&r, "enclave_id");
    if eid != Some(enclave_id) {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn post_edit_room(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, room_id)): Path<(i64, i64)>,
    axum::Form(form): axum::Form<RoomEditForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    assert_room_in_enclave(&state.chat, id, room_id).await?;
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    if let Err(e) = db::chat::update_room(
        &state.chat,
        room_id,
        name,
        form.topic.as_deref().filter(|s| !s.is_empty()),
    )
    .await
    {
        if is_unique_violation(&e) {
            return Ok(Redirect::to(&format!(
                "/enclave/{id}?error=room_name_taken&name={}",
                flash_name_param(name)
            )));
        }
        return Err(e.into());
    }
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn post_delete_room(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, room_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    assert_room_in_enclave(&state.chat, id, room_id).await?;
    db::chat::delete_room(&state.chat, room_id).await?;
    broadcast_to_enclave(
        &state,
        id,
        &ChatEvent::EnclaveRoomRemoved {
            enclave_id: id,
            room_id,
        },
    )
    .await;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

#[derive(Deserialize)]
pub struct RoomMemberForm {
    pub user_id: String,
}

pub async fn post_add_room_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, room_id)): Path<(i64, i64)>,
    axum::Form(form): axum::Form<RoomMemberForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    assert_room_in_enclave(&state.chat, id, room_id).await?;
    let target = form.user_id.trim();
    if db::enclave::get_membership(&state.chat, id, target)
        .await?
        .is_none()
    {
        return Err(AppError::BadRequest("user is not an enclave member".into()));
    }
    db::chat::add_room_member(&state.chat, room_id, target).await?;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn post_remove_room_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, room_id, target)): Path<(i64, i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    assert_room_in_enclave(&state.chat, id, room_id).await?;
    db::chat::remove_room_member(&state.chat, room_id, &target).await?;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn get_invitations(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let invs = db::enclave::list_invitations_for_user(&state.chat, &user.id).await?;
    let (
        sidebar_categories,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    html(&crate::views::enclave::InvitationsPage {
        user: &user,
        invitations: &invs,
        sidebar_categories: &sidebar_categories,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}
