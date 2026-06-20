use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
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
    DiscoverPage, EnclaveBrandingPage, EnclaveInviteCandidate, EnclaveInviteCandidateState,
    EnclaveInviteRowResult, EnclaveInviteSearchFragment, EnclaveMemberView, EnclavePage,
    EnclaveSettingsPage,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// Resolve each membership row to an `EnclaveMemberView` carrying a
/// human-readable label (display_name when set, otherwise `@username`,
/// otherwise the raw user_id as a last resort).
pub(crate) async fn resolve_member_views(
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

/// LC-170: fan an enclave list-mutation event out to every connection
/// subscribed to the `enclave:{id}` topic (the landing page subscribes via
/// `data-lc-live-topic`). The WS send task re-renders the member/room list
/// OOB per recipient. Replaces the older per-member `broadcast_to_user` fan,
/// whose events rendered nothing and only reached the mutation's own subject.
fn broadcast_enclave_topic(state: &AppState, enclave_id: i64, event: &ChatEvent) {
    state
        .hub
        .broadcast_to_topic(&format!("enclave:{enclave_id}"), event);
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
        .route("/enclave/{id}/rate-limit", post(post_msg_rate_limit))
        .route("/enclave/{id}/coyote-mode", post(post_coyote_mode))
        .route("/enclave/{id}/shame-tags", post(post_shame_tags))
        .route("/enclave/{id}/bans/{user_id}/unban", post(post_unban))
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
        .route(
            "/enclave/{id}/branding",
            get(get_branding)
                .post(post_branding)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
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
) -> Result<Response, AppError> {
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
    // LC-143: open the room the user last had here (validated), else the
    // default (first) room. The redirect target comes from the set the user
    // can actually OPEN - the same accessibility `get_room` enforces
    // (site-admin god-mode, else public-in-enclave or explicit private
    // membership). Using the manage-view list would redirect an enclave owner
    // who is not a member of a private room straight into a 403.
    let openable =
        db::chat::list_rooms_in_enclave(&state.chat, id, &user.id, is_site_admin).await?;
    let last = db::enclave::get_last_room(&state.chat, &user.id, id).await?;
    let target = last
        .filter(|rid| openable.iter().any(|r| r.id == *rid))
        .or_else(|| openable.first().map(|r| r.id));
    if let Some(room_id) = target {
        return Ok(Redirect::to(&format!("/room/{room_id}")).into_response());
    }

    // LC-336: no openable room. The full landing menu was removed; render a
    // small placeholder pane. Create-chat lives on the sidebar `+` (managers)
    // and member management on the settings page (reached via the switcher gear).
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, Some(id)).await?;
    Ok(html(&EnclavePage {
        user: &user,
        enclave: &enclave,
        can_manage,
        flash_error: flash_message(flash.error.as_deref(), flash.name.as_deref()).as_deref(),
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })?
    .into_response())
}

pub async fn get_discover(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(flash): Query<FlashQuery>,
) -> Result<Html, AppError> {
    let enclaves = db::enclave::list_public_enclaves(&state.chat).await?;
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
    html(&DiscoverPage {
        user: &user,
        enclaves: &enclaves,
        flash_error: flash_message(flash.error.as_deref(), flash.name.as_deref()).as_deref(),
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

/// LC-217: form for `POST /enclave/{id}/rate-limit`. `burst` is a per-minute
/// burst counter. `0` clears the override (use the global cap). Upper
/// bound 10_000 is generous and only there to keep an accidental gigantic
/// value from being silently accepted; the HTML `<input>` matches with
/// `max="10000"`, so the bound here is defense in depth against forged
/// POSTs.
#[derive(Deserialize)]
pub struct MsgRateLimitForm {
    pub burst: String,
}

pub async fn post_msg_rate_limit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MsgRateLimitForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let burst: u32 = form
        .burst
        .trim()
        .parse()
        .map_err(|_| AppError::BadRequest("burst must be a non-negative integer".into()))?;
    if burst > 10_000 {
        return Err(AppError::BadRequest("burst exceeds 10000".into()));
    }
    db::enclave::set_msg_rate_limit_burst(&state.chat, id, burst).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

#[derive(Deserialize)]
pub struct CoyoteModeForm {
    /// "1" enables, anything else (incl. the toggle-off button's "0") disables.
    pub enabled: String,
}

/// LC-339: toggle "Coyote Mode" anti-spam for an enclave (manager-gated).
pub async fn post_coyote_mode(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<CoyoteModeForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let enabled = form.enabled.trim() == "1";
    db::enclave::set_coyote_mode(&state.chat, id, enabled).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

/// LC-342: toggle the shame-tag prototype for an enclave (manager-gated).
pub async fn post_shame_tags(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<CoyoteModeForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let enabled = form.enabled.trim() == "1";
    db::enclave::set_shame_tags_enabled(&state.chat, id, enabled).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

/// LC-340: lift an enclave ban (manager-gated). The user can then rejoin and
/// post again. Idempotent: unbanning a non-banned user is a no-op redirect.
pub async fn post_unban(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, target)): Path<(i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    db::enclave::unban_from_enclave(&state.chat, id, &target).await?;
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
    // LC-339: a user banned from this enclave (e.g. by Coyote Mode) cannot rejoin.
    if db::enclave::is_enclave_banned(&state.chat, id, &user.id).await? {
        return Err(AppError::Forbidden);
    }
    if db::enclave::get_membership(&state.chat, id, &user.id)
        .await?
        .is_some()
    {
        return Ok(Redirect::to(&format!("/enclave/{id}")));
    }
    db::enclave::add_member(&state.chat, id, &user.id, EnclaveRole::Member).await?;
    broadcast_enclave_topic(
        &state,
        id,
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
    // LC-339: a user banned from this enclave (e.g. by Coyote Mode) cannot rejoin.
    if db::enclave::is_enclave_banned(&state.chat, enclave.id, &user.id).await? {
        return Err(AppError::Forbidden);
    }
    if db::enclave::get_membership(&state.chat, enclave.id, &user.id)
        .await?
        .is_some()
    {
        return Ok(Redirect::to(&format!("/enclave/{}", enclave.id)));
    }
    db::enclave::add_member(&state.chat, enclave.id, &user.id, EnclaveRole::Member).await?;
    broadcast_enclave_topic(
        &state,
        enclave.id,
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
    broadcast_enclave_topic(
        &state,
        eid,
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

    // LC-340: resolve a display label per banned user for the ban-list section.
    let mut bans: Vec<crate::views::enclave::EnclaveBanView> = Vec::new();
    for b in db::enclave::list_enclave_bans(&state.chat, id).await? {
        let label = db::auth::find_user_by_id(&state.auth, &b.user_id)
            .await?
            .map(|r| match r.display_name.as_deref() {
                Some(n) if !n.trim().is_empty() => n.to_string(),
                _ => format!("@{}", r.username),
            })
            .unwrap_or_else(|| b.user_id.clone());
        bans.push(crate::views::enclave::EnclaveBanView {
            user_id: b.user_id,
            label,
            reason: b.reason,
            banned_at: b.banned_at,
        });
    }

    // LC-83: resolve groups for the enclave alongside their member
    // labels so the settings page can render the CRUD UI without
    // extra fetches per row.
    let raw_groups = db::user_groups::list_for_enclave(&state.chat, id).await?;
    let mut groups: Vec<crate::views::enclave::EnclaveGroupView> =
        Vec::with_capacity(raw_groups.len());
    for g in raw_groups {
        let ids = db::user_groups::list_member_ids(&state.chat, g.id).await?;
        let mut labels: Vec<String> = Vec::with_capacity(ids.len());
        for uid in &ids {
            let label = db::auth::find_user_by_id(&state.auth, uid)
                .await?
                .map(|r| match r.display_name.as_deref() {
                    Some(n) if !n.trim().is_empty() => n.to_string(),
                    _ => format!("@{}", r.username),
                })
                .unwrap_or_else(|| uid.clone());
            labels.push(label);
        }
        groups.push(crate::views::enclave::EnclaveGroupView {
            id: g.id,
            name: g.name,
            member_count: ids.len() as i64,
            member_labels: labels,
        });
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
    ) = super::load_chrome(&state, &user, Some(id)).await?;
    html(&EnclaveSettingsPage {
        user: &user,
        enclave: &enclave,
        members: &member_views,
        bans: &bans,
        groups: &groups,
        emojis: &emojis,
        can_delete,
        flash_error: flash_message(flash.error.as_deref(), flash.name.as_deref()).as_deref(),
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
    let topic = format!("enclave:{id}");
    for fm in former_members {
        state.hub.broadcast_to_user(
            &fm.user_id,
            &ChatEvent::EnclaveMemberRemoved {
                enclave_id: id,
                user_id: fm.user_id.clone(),
            },
        );
        // LC-176: the enclave is gone; drop every member's subscription to its
        // now-defunct topic so no stale tab keeps receiving its events.
        state.hub.unsubscribe_user_from_topic(&fm.user_id, &topic);
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
    // an enclave doesn't dirty any category row. LC-80: stars are
    // per-user; scrub the leaving user's stars on rooms they're
    // about to lose access to so a re-join starts fresh.
    let lost_rooms = db::chat::list_rooms_in_enclave(&state.chat, id, &user.id, false).await?;
    let lost_ids: Vec<i64> = lost_rooms.iter().map(|r| r.id).collect();
    db::enclave::remove_member(&state.chat, id, &user.id).await?;
    db::starred_rooms::forget_rooms(&state.auth, &user.id, &lost_ids).await?;
    broadcast_enclave_topic(
        &state,
        id,
        &ChatEvent::EnclaveMemberRemoved {
            enclave_id: id,
            user_id: user.id.clone(),
        },
    );
    // LC-176: the leaver no longer has access; drop their subscription to the
    // enclave topic so their open tabs stop receiving its events.
    state
        .hub
        .unsubscribe_user_from_topic(&user.id, &format!("enclave:{id}"));
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
    // LC-351: never let a role change touch the owner. The template hides the
    // owner's role control, but update_role's UPDATE has no owner clause, so a
    // forged POST would otherwise demote the owner and leave the enclave with
    // zero owners (then un-transferable and un-deletable). Mirror post_kick.
    let Some(target_m) = db::enclave::get_membership(&state.chat, id, &target).await? else {
        return Err(AppError::NotFound);
    };
    if matches!(target_m.role, EnclaveRole::Owner) {
        return Err(AppError::BadRequest(
            "cannot change the owner's role; transfer ownership first".into(),
        ));
    }
    db::enclave::update_role(&state.chat, id, &target, new_role).await?;
    broadcast_enclave_topic(
        &state,
        id,
        &ChatEvent::EnclaveMemberRoleChanged {
            enclave_id: id,
            user_id: target.clone(),
        },
    );
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
    // per-user. LC-80: stars are per-user; scrub the kicked user's
    // stars on rooms they're about to lose access to.
    let lost_rooms = db::chat::list_rooms_in_enclave(&state.chat, id, &target, false).await?;
    let lost_ids: Vec<i64> = lost_rooms.iter().map(|r| r.id).collect();
    db::enclave::remove_member(&state.chat, id, &target).await?;
    db::starred_rooms::forget_rooms(&state.auth, &target, &lost_ids).await?;
    broadcast_enclave_topic(
        &state,
        id,
        &ChatEvent::EnclaveMemberRemoved {
            enclave_id: id,
            user_id: target.clone(),
        },
    );
    // LC-176: the kicked user has lost access; drop their subscription to the
    // enclave topic so their open tabs stop receiving its events.
    state
        .hub
        .unsubscribe_user_from_topic(&target, &format!("enclave:{id}"));
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
    broadcast_enclave_topic(
        &state,
        id,
        &ChatEvent::EnclaveRoomAdded {
            enclave_id: id,
            room_id,
        },
    );
    // LC-400: land the creator directly in the new room. Redirecting to
    // `/enclave/{id}` instead bounced through `get_landing`, which opens the
    // user's last-visited / first openable room (not the one just created), so
    // creating a room felt like a no-op. The creator can always open the new
    // room: a public room is openable to every enclave member, and a private
    // room added the creator as a member just above.
    Ok(Redirect::to(&format!("/room/{room_id}")))
}

#[derive(Deserialize)]
pub struct RoomEditForm {
    pub name: String,
    pub topic: Option<String>,
    /// LC-86: long-form description shown on `/room/{id}/info`.
    /// Optional; empty string clears.
    pub description: Option<String>,
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
    // LC-86: separate column. Always run the update so an empty
    // string clears it.
    db::chat::set_room_description(
        &state.chat,
        room_id,
        form.description.as_deref().filter(|s| !s.is_empty()),
    )
    .await?;
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
    broadcast_enclave_topic(
        &state,
        id,
        &ChatEvent::EnclaveRoomRemoved {
            enclave_id: id,
            room_id,
        },
    );
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
    // LC-80: drop the removed user's star on this room so re-adding
    // them doesn't resurrect the old star.
    db::starred_rooms::forget_room(&state.auth, &target, room_id).await?;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn get_invitations(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let invs = db::enclave::list_invitations_for_user(&state.chat, &user.id).await?;
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
    html(&crate::views::enclave::InvitationsPage {
        user: &user,
        invitations: &invs,
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

// Per-enclave branding (LC-96) --------------------------------------------

#[derive(Deserialize, Default)]
pub struct EnclaveBrandingQuery {
    pub saved: Option<i64>,
}

async fn render_enclave_branding_page(
    state: &AppState,
    user: &User,
    enclave_id: i64,
    saved: bool,
    error: Option<String>,
) -> Result<Html, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, enclave_id).await? else {
        return Err(AppError::NotFound);
    };
    require_manage(state, user, enclave_id).await?;
    let branding =
        db::branding::resolve(&state.chat, db::branding::Scope::Enclave(enclave_id)).await?;
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, Some(enclave_id)).await?;
    html(&EnclaveBrandingPage {
        user,
        enclave: &enclave,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        primary_color: branding.primary_color,
        accent_color: branding.accent_color,
        login_heading: branding.login_heading,
        login_body: branding.login_body,
        has_logo: branding.logo_upload_id.is_some(),
        saved,
        error,
    })
}

pub async fn get_branding(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<EnclaveBrandingQuery>,
) -> Result<Html, AppError> {
    render_enclave_branding_page(&state, &user, id, q.saved.is_some(), None).await
}

pub async fn post_branding(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    require_manage(&state, &user, id).await?;
    let form = match super::branding::parse_branding_multipart(&state, &user.id, multipart).await? {
        Ok(f) => f,
        Err(msg) => {
            return Ok(
                render_enclave_branding_page(&state, &user, id, false, Some(msg))
                    .await?
                    .into_response(),
            );
        }
    };
    // Fall back to the RESOLVED branding (this enclave's row if it
    // exists, otherwise the global row) rather than hard-coded
    // defaults. The GET form renders the same resolved values, so a
    // partial save (e.g. editing only the heading) keeps whatever
    // colors the operator was looking at instead of silently
    // reverting them to the built-in blue.
    let existing = db::branding::resolve(&state.chat, db::branding::Scope::Enclave(id)).await?;
    let logo_upload_id = form.new_logo_id.or(existing.logo_upload_id);
    // LC-142 favicons are global-only; preserve whatever the row holds
    // (always None for enclaves) so the enclave form never clears it.
    let favicon_upload_id = existing.favicon_upload_id;
    let primary = form.primary_color.unwrap_or(existing.primary_color);
    let accent = form.accent_color.unwrap_or(existing.accent_color);
    if !db::branding::is_valid_hex_color(&primary) || !db::branding::is_valid_hex_color(&accent) {
        return Ok(render_enclave_branding_page(
            &state,
            &user,
            id,
            false,
            Some("Colors must be #rgb or #rrggbb hex".into()),
        )
        .await?
        .into_response());
    }
    let heading = form.login_heading.unwrap_or(existing.login_heading);
    let body = form.login_body.unwrap_or(existing.login_body);
    db::branding::upsert(
        &state.chat,
        db::branding::Scope::Enclave(id),
        logo_upload_id,
        favicon_upload_id,
        &primary,
        &accent,
        &heading,
        &body,
        &user.id,
    )
    .await?;
    db::moderation::log_mod_action(
        &state.chat,
        "branding_set",
        "",
        &user.id,
        None,
        None,
        Some(&format!("enclave={id}")),
    )
    .await?;
    Ok(Redirect::to(&format!("/enclave/{id}/branding?saved=1")).into_response())
}
