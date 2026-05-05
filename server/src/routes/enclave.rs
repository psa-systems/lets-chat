use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct FlashQuery {
    #[serde(default)]
    pub error: Option<String>,
}

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::enclave::EnclaveRole;
use crate::models::User;
use crate::perms::enclave_can_manage;
use crate::perms::{enclave_can_delete, enclave_can_manage_admins};
use crate::state::AppState;
use crate::views::enclave::{DiscoverPage, EnclavePage, EnclaveSettingsPage};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

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
        .route(
            "/enclave/{id}/invite-code",
            post(post_invite_code).delete(delete_invite_code),
        )
        .route("/enclave/{id}/invite", post(post_invite))
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
            let msg = format!("Enclave \"{name}\" already exists. Pick a different name.");
            return Ok(Redirect::to(&format!(
                "/enclaves/discover?error={}",
                percent_encoding::utf8_percent_encode(&msg, percent_encoding::NON_ALPHANUMERIC)
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
    let rooms = db::chat::list_rooms_in_enclave(&state.chat, id, &user.id, can_manage).await?;
    let (sidebar_rooms, sidebar_peers, switcher) =
        super::load_chrome(&state, &user, Some(id)).await?;
    html(&EnclavePage {
        user: &user,
        enclave: &enclave,
        members: &members,
        rooms: &rooms,
        can_manage,
        flash_error: flash.error.as_deref(),
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
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    html(&DiscoverPage {
        user: &user,
        enclaves: &enclaves,
        flash_error: flash.error.as_deref(),
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
    pub username: String,
}

pub async fn post_invite(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<InviteForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let Some(target) = db::auth::find_user_by_username(&state.auth, form.username.trim()).await?
    else {
        return Err(AppError::BadRequest("user not found".into()));
    };
    if db::enclave::get_membership(&state.chat, id, &target.id)
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest("user is already a member".into()));
    }
    if let Err(e) = db::enclave::create_invitation(&state.chat, id, &target.id, &user.id).await {
        if !matches!(&e, sqlx::Error::Database(d) if d.is_unique_violation()) {
            return Err(e.into());
        }
    }
    state.hub.broadcast_to_user(
        &target.id,
        &ChatEvent::EnclaveInvitationCreated {
            invitee_id: target.id.clone(),
        },
    );
    Ok(Redirect::to(&format!("/enclave/{id}")))
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
    let (sidebar_rooms, sidebar_peers, switcher) =
        super::load_chrome(&state, &user, Some(id)).await?;
    html(&EnclaveSettingsPage {
        user: &user,
        enclave: &enclave,
        members: &members,
        can_delete,
        flash_error: flash.error.as_deref(),
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
            let msg = format!("Enclave \"{name}\" already exists. Pick a different name.");
            return Ok(Redirect::to(&format!(
                "/enclave/{id}/settings?error={}",
                percent_encoding::utf8_percent_encode(&msg, percent_encoding::NON_ALPHANUMERIC)
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
    pub room_type: String,
}

pub async fn post_create_room(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<RoomForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    if !matches!(form.room_type.as_str(), "public" | "private") {
        return Err(AppError::BadRequest("invalid room_type".into()));
    }
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
    let room_id = match db::chat::create_room(
        &state.chat,
        name,
        form.topic.as_deref().filter(|s| !s.is_empty()),
        &form.room_type,
        invite_code.as_deref(),
        Some(id),
    )
    .await
    {
        Ok(rid) => rid,
        Err(e) if is_unique_violation(&e) => {
            let msg = format!("Room \"{name}\" already exists. Pick a different name.");
            return Ok(Redirect::to(&format!(
                "/enclave/{id}?error={}",
                percent_encoding::utf8_percent_encode(&msg, percent_encoding::NON_ALPHANUMERIC)
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
            let msg = format!("Room \"{name}\" already exists. Pick a different name.");
            return Ok(Redirect::to(&format!(
                "/enclave/{id}?error={}",
                percent_encoding::utf8_percent_encode(&msg, percent_encoding::NON_ALPHANUMERIC)
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
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    html(&crate::views::enclave::InvitationsPage {
        user: &user,
        invitations: &invs,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}
