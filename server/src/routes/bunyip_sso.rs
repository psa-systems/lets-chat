//! LC-22: pure-RP "Sign in with Bunyip" SSO routes.
//!
//! The cutover deletes lets-chat's local password path entirely; Bunyip is the
//! sole sign-in surface. Only two handlers exist here:
//!
//! - `GET /auth/bunyip/start`: generate PKCE state, persist the pending row,
//!   302 to bunyip-api's authorize endpoint.
//! - `GET /auth/bunyip/callback`: consume the pending row, exchange the code,
//!   verify the id_token, resolve-or-provision the local user, issue the
//!   lets-chat session cookie via the SAME helpers a password login used to.
//!
//! See `docs/lets-chat/sso/bunyip-only/01-architecture.md` (single flow) and
//! `03-account-provisioning.md` (the decision tree the callback applies).

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::db;
use crate::error::AppError;
use crate::oidc::{new_pkce_pair, new_random_token};
use crate::routes::auth::build_session_cookie;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

fn err_dance(reason: &str) -> Response {
    tracing::warn!(target: "bunyip_sso", reason, "sso callback rejected");
    Redirect::to("/login?sso_error=dance").into_response()
}

fn err_op(reason: &str) -> Response {
    tracing::warn!(target: "bunyip_sso", reason, "sso callback rejected (op-side)");
    Redirect::to("/login?sso_error=op").into_response()
}

/// `GET /auth/bunyip/start` - initiate the SSO dance. Generates a fresh PKCE
/// pair + nonce + state, persists them in `oidc_pending`, then 302s the
/// browser to the bunyip authorize endpoint.
pub async fn get_start(State(state): State<AppState>) -> Response {
    let client = state.bunyip_sso_client().clone();
    let state_token = new_random_token();
    let nonce = new_random_token();
    let (verifier, challenge) = new_pkce_pair();
    if let Err(e) = db::oidc_pending::insert(&state.auth, &state_token, &verifier, &nonce).await {
        tracing::error!(target: "bunyip_sso", error = %e, "oidc_pending insert failed");
        return err_dance("pending insert failed");
    }
    let url = client.authorize_url(&state_token, &nonce, &challenge);
    Redirect::to(&url).into_response()
}

/// `GET /auth/bunyip/callback` - complete the SSO dance.
///
/// Order of operations mirrors the spec in
/// `docs/lets-chat/sso/bunyip-only/01-architecture.md` step 6:
/// 1. Consume the `oidc_pending` row.
/// 2. Exchange the code for tokens.
/// 3. Verify the id_token (signature + iss + aud + exp + nonce).
/// 4. Fetch `/oauth2/userinfo`.
/// 5. Resolve-or-provision the local user.
/// 6. Issue a session cookie via the existing helper.
pub async fn get_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Response {
    if let Some(e) = params.error.as_deref() {
        tracing::warn!(target: "bunyip_sso", reason = "op error", op_error = %e, op_desc = ?params.error_description);
        return err_op(e);
    }
    let (Some(code), Some(state_token)) = (params.code, params.state) else {
        return err_dance("callback missing code/state");
    };
    let client = state.bunyip_sso_client().clone();

    let pending = match db::oidc_pending::take(&state.auth, &state_token).await {
        Ok(Some(p)) => p,
        Ok(None) => return err_dance("pending row missing/expired"),
        Err(e) => {
            tracing::error!(target: "bunyip_sso", error = %e, "oidc_pending take failed");
            return err_dance("pending row read failed");
        }
    };

    let tokens = match client.exchange_code(&code, &pending.code_verifier).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target: "bunyip_sso", error = %e, "token exchange failed");
            return err_op("token exchange");
        }
    };

    let id_claims = match client
        .verify_id_token(&tokens.id_token, &pending.nonce)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(target: "bunyip_sso", error = %e, "id_token verify failed");
            return err_dance("id_token verify");
        }
    };

    let userinfo = match client.user_info(&tokens.access_token).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(target: "bunyip_sso", error = %e, "userinfo fetch failed");
            return err_op("userinfo");
        }
    };

    if userinfo.sub != id_claims.sub {
        tracing::warn!(target: "bunyip_sso", id_sub = %id_claims.sub, ui_sub = %userinfo.sub, "userinfo sub mismatch");
        return err_dance("userinfo sub mismatch");
    }

    let user_id = match resolve_or_provision_user(&state, &id_claims.sub, &userinfo).await {
        Ok(id) => id,
        Err(ResolveError::Banned) => {
            tracing::warn!(target: "bunyip_sso", sub = %id_claims.sub, "sso reject: banned user");
            return Redirect::to("/login?sso_error=banned").into_response();
        }
        Err(ResolveError::BotConflict) => {
            tracing::warn!(target: "bunyip_sso", sub = %id_claims.sub, "sso reject: sub attached to bot row");
            return err_dance("bot conflict");
        }
        Err(ResolveError::Database(e)) => {
            tracing::error!(target: "bunyip_sso", error = %e, "resolve_or_provision_user failed");
            return Redirect::to("/login?sso_error=internal").into_response();
        }
        Err(ResolveError::App(e)) => {
            tracing::error!(target: "bunyip_sso", error = ?e, "resolve_or_provision_user failed");
            return Redirect::to("/login?sso_error=internal").into_response();
        }
    };

    if let Err(e) =
        mirror_bunyip_admin_role(&state, &user_id, id_claims.bunyip_role.as_deref()).await
    {
        tracing::error!(target: "bunyip_sso", error = %e, user_id = %user_id, "bunyip_role mirror failed");
        return Redirect::to("/login?sso_error=internal").into_response();
    }

    let trust_proxy = crate::auth::proxy_headers_trusted(&state.settings).await;
    let (ua, ip) = crate::auth::extract_session_origin(&headers, trust_proxy);
    let token = match db::auth::create_session_with_origin(
        &state.auth,
        &user_id,
        ua.as_deref(),
        ip.as_deref(),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(target: "bunyip_sso", error = %e, "session create failed");
            return Redirect::to("/login?sso_error=internal").into_response();
        }
    };
    tracing::info!(target: "bunyip_sso", path = "sso", user_id = %user_id, "session created");
    let cookie = build_session_cookie(state.cookies_secure(), token);
    let jar = jar.add(cookie);
    (jar, Redirect::to("/")).into_response()
}

#[derive(Debug)]
enum ResolveError {
    Banned,
    BotConflict,
    Database(sqlx::Error),
    App(AppError),
}

impl From<sqlx::Error> for ResolveError {
    fn from(e: sqlx::Error) -> Self {
        ResolveError::Database(e)
    }
}

impl From<AppError> for ResolveError {
    fn from(e: AppError) -> Self {
        ResolveError::App(e)
    }
}

/// `users` resolver for a verified Bunyip identity. See
/// `docs/lets-chat/sso/bunyip-only/03-account-provisioning.md` §3.1.
async fn resolve_or_provision_user(
    state: &AppState,
    sub: &str,
    userinfo: &crate::oidc::UserInfo,
) -> Result<String, ResolveError> {
    if let Some((id, is_banned, is_bot)) =
        db::auth::get_user_auth_flags_by_bunyip_sub(&state.auth, sub).await?
    {
        if is_banned {
            return Err(ResolveError::Banned);
        }
        if is_bot {
            return Err(ResolveError::BotConflict);
        }
        return Ok(id);
    }

    // Fresh sub: auto-provision a users row.
    let username = pick_username(state, userinfo).await?;
    let display_name = userinfo.name.as_deref().filter(|s| !s.trim().is_empty());
    let email = userinfo.email.as_deref().filter(|s| !s.trim().is_empty());
    let id =
        db::auth::create_user_from_bunyip(&state.auth, &username, sub, display_name, email).await?;

    promote_if_first_user(&state.auth, &id).await?;
    Ok(id)
}

/// Username pick: prefer `preferred_username`, fall back to the email
/// local-part. Try up to 5 collision suffixes (`-2` ... `-5`) before bailing.
async fn pick_username(
    state: &AppState,
    info: &crate::oidc::UserInfo,
) -> Result<String, ResolveError> {
    let base = info
        .preferred_username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            info.email
                .as_deref()
                .and_then(|e| e.split('@').next())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("user")
        .to_string();
    let base = sanitize_username(&base);
    if !db::auth::username_exists(&state.auth, &base).await? {
        return Ok(base);
    }
    for n in 2..=5u32 {
        let candidate = format!("{base}-{n}");
        if !db::auth::username_exists(&state.auth, &candidate).await? {
            return Ok(candidate);
        }
    }
    Err(ResolveError::App(AppError::Internal(
        "could not pick a unique username after 5 attempts".into(),
    )))
}

fn sanitize_username(s: &str) -> String {
    let mut out: String = s
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(*c, '_' | '-' | '.'))
        .collect();
    if out.is_empty() {
        out.push_str("user");
    }
    out.truncate(64);
    out
}

/// LC-413: mirror Bunyip's platform role onto the lets-chat row.
///
/// Bunyip is the source of truth for the top role. The Bunyip
/// `admin` claim grants the lets-chat `admin` role; the
/// `subscriber` claim (or any unknown future value) demotes a stale
/// local `admin` back down to `user`. The intermediate `moderator`
/// role is a lets-chat-internal grant and stays untouched in both
/// directions; only the admin/user mirror runs here.
///
/// The first-user-to-admin promotion (`promote_if_first_user`) still
/// runs in the auto-provision path so a brand-new deployment whose
/// only user is a Bunyip subscriber still ends up with an admin.
/// This reconcile runs AFTER the promotion, so a Bunyip subscriber
/// who was promoted on first sign-in stays admin; a returning
/// subscriber who was never an admin stays a user; only the
/// admin-then-demoted case downgrades, which is intentional.
async fn mirror_bunyip_admin_role(
    state: &AppState,
    user_id: &str,
    bunyip_role: Option<&str>,
) -> Result<(), sqlx::Error> {
    let current = db::auth::get_user_role(&state.auth, user_id).await?;
    let current = current.as_deref().unwrap_or("user");
    let desired = match bunyip_role {
        Some("admin") => Some("admin"),
        _ if current == "admin" => Some("user"),
        _ => None,
    };
    if let Some(next) = desired {
        if next != current {
            tracing::info!(
                target: "bunyip_sso",
                user_id = %user_id,
                from = %current,
                to = %next,
                bunyip_role = ?bunyip_role,
                "mirroring lets-chat role from Bunyip claim"
            );
            db::auth::set_user_role(&state.auth, user_id, next).await?;
        }
    }
    Ok(())
}

/// First-non-bot user gets the admin role. Mirrors the prior password-path
/// `promote_first_user_to_admin` in `routes/auth.rs`.
async fn promote_if_first_user(
    pool: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_bot = 0")
        .fetch_one(&mut *tx)
        .await?;
    let promoted = count == 1;
    if promoted {
        sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(promoted)
}
