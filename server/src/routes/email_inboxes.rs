//! LC-77: per-room email-ingress inbox management routes.
//! Cookie-authenticated, gated to room moderators. Mirrors
//! `crate::routes::webhooks` for create / list / revoke. The
//! resolve / post path lives in `crate::email_ingress::poll`; this
//! module is the admin surface only.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::email_inboxes::{EmailInboxRowView, RoomEmailInboxesPage};
use crate::views::{html, Html};

async fn require_room_manage(
    state: &AppState,
    user: &crate::models::User,
    room_id: i64,
) -> Result<(), AppError> {
    if db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Resolve the ingress domain from `imap_inbox_config`. Returns `None` if
/// either the secret key is missing (no creds can be decrypted) or the
/// row's `ingress_domain` column is unset.
async fn resolve_ingress_domain(state: &AppState) -> Result<Option<String>, AppError> {
    let Some(key) = state.secret_key.as_ref() else {
        return Ok(None);
    };
    let Some(cfg) = db::imap_config::read(&state.settings, key.as_ref())
        .await
        .map_err(|e| AppError::Internal(format!("imap_config read: {e}")))?
    else {
        return Ok(None);
    };
    Ok(cfg.ingress_domain)
}

async fn render_page(
    state: &AppState,
    user: &crate::models::User,
    room_id: i64,
    new_address: Option<String>,
    error: Option<String>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let current_enclave = super::enclave_for_room(state, room_id).await?;
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, current_enclave).await?;
    let inboxes: Vec<EmailInboxRowView> = db::email_inbox::list_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .map(|w| EmailInboxRowView {
            id: w.id,
            name: w.name,
            avatar_url: w.avatar_url,
            created_at: w.created_at,
            last_used_at: w.last_used_at,
            revoked: w.revoked,
        })
        .collect();
    let ingress_domain = resolve_ingress_domain(state).await?;
    let (available, missing_setting) = match (state.secret_key.is_some(), ingress_domain.as_deref())
    {
        (false, _) => (false, Some("LETS_CHAT_SECRET_KEY env var")),
        (true, None) => (false, Some("admin settings: IMAP ingress domain")),
        (true, Some(_)) => (true, None),
    };
    html(&RoomEmailInboxesPage {
        user,
        room: &room,
        available,
        missing_setting,
        inboxes: &inboxes,
        new_address,
        error,
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

/// GET /room/{id}/email-inboxes
pub async fn get_email_inboxes(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    render_page(&state, &user, room_id, None, None).await
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub name: String,
    #[serde(default)]
    pub avatar_url: String,
}

/// POST /room/{id}/email-inboxes - create an inbox and reveal its address once.
pub async fn post_email_inboxes(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<Response, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    let Some(key) = state.secret_key.as_ref() else {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some(
                "Email inboxes need a server secret key (LETS_CHAT_SECRET_KEY).".into(),
            ),
        )
        .await?
        .into_response());
    };
    let Some(ingress_domain) = resolve_ingress_domain(&state).await? else {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some(
                "Email inboxes need the IMAP ingress domain configured in admin settings.".into(),
            ),
        )
        .await?
        .into_response());
    };
    let name = form.name.trim();
    if name.is_empty() {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some("Inbox name is required.".into()),
        )
        .await?
        .into_response());
    }
    let avatar = form.avatar_url.trim();
    let avatar = (!avatar.is_empty()).then_some(avatar);
    if let Some(a) = avatar {
        if !(a.starts_with("https://") || a.starts_with("http://")) {
            return Ok(render_page(
                &state,
                &user,
                room_id,
                None,
                Some("Avatar URL must be an http(s) URL.".into()),
            )
            .await?
            .into_response());
        }
    }

    let secret = crate::auth::generate_api_token();
    let hash = crate::auth::hash_api_token(key, &secret);
    db::email_inbox::insert(&state.chat, room_id, name, avatar, &hash, &user.id).await?;
    let address = format!("{secret}@{ingress_domain}");
    Ok(render_page(&state, &user, room_id, Some(address), None)
        .await?
        .into_response())
}

/// POST /room/{id}/email-inboxes/{id}/revoke
pub async fn post_revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, inbox_id)): Path<(i64, i64)>,
) -> Result<Redirect, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    db::email_inbox::revoke(&state.chat, inbox_id, room_id).await?;
    Ok(Redirect::to(&format!("/room/{room_id}/email-inboxes")))
}
