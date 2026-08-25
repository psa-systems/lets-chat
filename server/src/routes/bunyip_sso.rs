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

use axum::extract::{Form, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use time::Duration;

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
    // LC-698: an empty subject is the UNLINKED marker in `users.bunyip_sub`, so
    // accepting one would resolve the login to an arbitrary unlinked account.
    if id_claims.sub.trim().is_empty() {
        return err_dance("empty sub claim");
    }

    let user_id = match resolve_or_provision_user(&state, &id_claims.sub, &userinfo).await {
        Ok(id) => id,
        Err(e) => {
            let code = sso_error_code(&e);
            match &e {
                ResolveError::Database(err) => {
                    tracing::error!(target: "bunyip_sso", error = %err, "resolve_or_provision_user failed");
                }
                ResolveError::App(err) => {
                    tracing::error!(target: "bunyip_sso", error = ?err, "resolve_or_provision_user failed");
                }
                other => {
                    tracing::warn!(target: "bunyip_sso", sub = %id_claims.sub, code, reason = ?other, "sso reject");
                }
            }
            return Redirect::to(&format!("/login?sso_error={code}")).into_response();
        }
    };

    // LC-733: keep this login's access token so the desktop updater can pull
    // the membership-gated binaries as this user (see routes::desktop_update).
    // Stored before the suspicious-login gate below because both completion
    // paths need it; an entry with no session behind it is unreachable, since
    // the route that serves it requires one.
    super::desktop_update::remember(&user_id, &tokens.access_token, tokens.expires_in);

    if let Err(e) =
        mirror_bunyip_admin_role(&state, &user_id, id_claims.bunyip_role.as_deref()).await
    {
        tracing::error!(target: "bunyip_sso", error = %e, user_id = %user_id, "bunyip_role mirror failed");
        return Redirect::to("/login?sso_error=internal").into_response();
    }

    // LC-627: bunyip is the authority for identity, so a bunyip-verified email
    // counts as verified here too. Provisioning never stamped `email_verified_at`,
    // which left every SSO user "unverified" to the features gated on it (notably
    // the remote-control consent gate, which then silently refused every request).
    // Stamp it now, idempotently, so new AND already-provisioned users are fixed on
    // their next login. Best-effort: a failure here must not block a valid login.
    // LC-698: only when the row's stored address IS the one bunyip vouched for -
    // otherwise a self-service profile email would stamp itself verified and
    // become an adoption target for the next login on that address.
    if userinfo.email_verified == Some(true) {
        if let Some(email) = userinfo
            .email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if let Err(e) =
                db::auth::mark_email_verified_if_unset(&state.auth, &user_id, email).await
            {
                tracing::warn!(target: "bunyip_sso", error = %e, user_id = %user_id, "email_verified stamp failed");
            }
        }
    }

    // LC-762: bunyip is the authority for identity (see the role mirror and the
    // email stamp above), so refresh the stored display name from the fresh
    // `name` claim on every login. The resolver returns an existing linked row
    // untouched, so a name changed at the IdP after the account was provisioned
    // stayed stale forever otherwise. Best-effort, like the stamps above: a
    // failure must not block a valid login, and a blank/absent claim is skipped
    // so a login never blanks an existing name. The username is deliberately NOT
    // synced - it is the mention handle, is uniqueness-constrained, and may carry
    // a provisioning collision suffix.
    if let Some(name) = userinfo
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(e) = db::auth::sync_bunyip_display_name(&state.auth, &user_id, name).await {
            tracing::warn!(target: "bunyip_sso", error = %e, user_id = %user_id, "display_name sync failed");
        }
    }

    let trust_proxy = crate::auth::proxy_headers_trusted(&state.settings).await;
    let (ua, ip) = crate::auth::extract_session_origin(&headers, trust_proxy);

    // LC-587: when the suspicious-login gate is enabled, assess the login
    // BEFORE minting a session. A suspicious login (new country and/or new
    // device) is withheld: a single-use code is emailed and an interstitial
    // asks the user to approve it. A cleared login falls through, mints the
    // session, and records its country/device as the new baseline. The gate is
    // opt-in (LOGIN_APPROVAL_ENABLED); when off, the original LC-580 alert-only
    // behaviour below runs unchanged.
    if state.login_approval_enabled {
        let device_id = read_device_id(&jar);
        match crate::login_approval::guard(
            &state,
            &user_id,
            ip.as_deref(),
            ua.as_deref(),
            device_id.as_deref(),
        )
        .await
        {
            crate::login_approval::LoginGate::Challenge { token } => {
                return render_approval_page(&state, &token, None);
            }
            crate::login_approval::LoginGate::Clear {
                country,
                device_hash,
            } => {
                return complete_login(
                    &state,
                    &jar,
                    &user_id,
                    ua.as_deref(),
                    ip.as_deref(),
                    country.as_deref(),
                    device_hash,
                )
                .await;
            }
        }
    }

    // Gate off (LC-580 path): mint the session and fire the detached,
    // best-effort new-login-location alert so the geoip lookup + any SMTP send
    // never add latency to the login redirect.
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
    let alert_state = state.clone();
    let alert_user_id = user_id.clone();
    let alert_ip = ip.clone();
    let alert_ua = ua.clone();
    tokio::spawn(async move {
        crate::login_alert::maybe_alert(
            &alert_state,
            &alert_user_id,
            alert_ip.as_deref(),
            alert_ua.as_deref(),
        )
        .await;
    });
    let cookie = build_session_cookie(state.cookies_secure(), token);
    let jar = jar.add(cookie);
    (jar, Redirect::to("/")).into_response()
}

#[derive(Debug)]
enum ResolveError {
    Banned,
    BotConflict,
    /// LC-618 + LC-698: the verified email is owned by a row this login must not
    /// adopt (already linked to a different subject, a bot row, or a banned
    /// row), so adopting would be a takeover and provisioning would collide on
    /// `UNIQUE(users.email)`. Surfaced to the user as an actionable message
    /// rather than an opaque internal error; an admin resolves a rotated sub by
    /// unlinking the row (`POST /admin/users/{id}/unlink-sso`).
    IdentityConflict,
    Database(sqlx::Error),
    App(AppError),
}

/// The `?sso_error=` code a resolver failure lands the browser on. One mapping,
/// so the code the user actually sees is testable without driving the whole
/// OIDC dance.
fn sso_error_code(e: &ResolveError) -> &'static str {
    match e {
        ResolveError::Banned => "banned",
        ResolveError::BotConflict => "dance",
        ResolveError::IdentityConflict => "identity_conflict",
        ResolveError::Database(_) | ResolveError::App(_) => "internal",
    }
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

/// LC-618: true when a sqlx error is a UNIQUE violation on `users.email`
/// specifically (not `username` or `bunyip_sub`), so the provision path can map
/// an email collision to an actionable identity conflict instead of an opaque
/// internal error.
fn is_email_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation() && d.message().contains("users.email"))
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

    // LC-588 + LC-698: the sub did not match above, but a users row may already
    // own the same VERIFIED email. Exactly one shape is adoptable: an UNLINKED,
    // verified-email, non-bot, non-banned row (LC-588's pre-SSO password user).
    // Every other shape is refused, because the local email is a self-service
    // string and must never re-point an identity at someone else's account.
    // LC-618 briefly relinked a row whose bunyip_sub had ROTATED; that silent
    // relink was the takeover primitive LC-698 removes, so a rotated sub is now
    // an explicit conflict that an admin resolves by unlinking the row.
    if userinfo.email_verified == Some(true) {
        if let Some(email) = userinfo
            .email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if let Some(owner) = db::auth::find_email_owner(&state.auth, email).await? {
                if owner.is_banned || owner.is_bot {
                    tracing::warn!(
                        target: "bunyip_sso", sub, owner_id = %owner.id,
                        banned = owner.is_banned, bot = owner.is_bot,
                        "sso adoption refused: verified email owned by a banned/bot row",
                    );
                    return Err(ResolveError::IdentityConflict);
                }
                if !owner.email_verified {
                    // The address was set self-service and never verified, so that
                    // row has no claim on it while the OP vouches for this login.
                    // Release it (otherwise the provision below collides on
                    // UNIQUE(users.email)) and provision a fresh account. Releasing
                    // also stops the squatting row from receiving mail addressed to
                    // this user.
                    tracing::warn!(
                        target: "bunyip_sso", sub, owner_id = %owner.id,
                        "releasing unverified profile email from another row; provisioning instead of adopting",
                    );
                    db::auth::set_user_email(&state.auth, &owner.id, None).await?;
                } else if let Some(other_sub) = owner.bunyip_sub.as_deref() {
                    tracing::warn!(
                        target: "bunyip_sso", sub, owner_id = %owner.id, other_sub,
                        "sso adoption refused: verified email already linked to a different subject",
                    );
                    return Err(ResolveError::IdentityConflict);
                } else if db::auth::link_bunyip_sub(&state.auth, &owner.id, sub).await? {
                    return Ok(owner.id);
                } else {
                    // Lost a race with a concurrent link: refuse rather than
                    // provision a duplicate that collides on the email.
                    tracing::warn!(
                        target: "bunyip_sso", sub, owner_id = %owner.id,
                        "sso adoption refused: row stopped being linkable mid-login",
                    );
                    return Err(ResolveError::IdentityConflict);
                }
            }
        }
    }

    // Fresh sub with no adoptable verified-email row: provision. If an email is
    // still owned by a row we did not adopt above (an unverified email on the
    // OP side whose address a local row holds), the INSERT collides on
    // UNIQUE(users.email). Surface that as an actionable identity conflict
    // rather than an opaque sso_error=internal (LC-618).
    let username = pick_username(state, userinfo).await?;
    let display_name = userinfo.name.as_deref().filter(|s| !s.trim().is_empty());
    let email = userinfo.email.as_deref().filter(|s| !s.trim().is_empty());
    let id =
        match db::auth::create_user_from_bunyip(&state.auth, &username, sub, display_name, email)
            .await
        {
            Ok(id) => id,
            Err(e) if is_email_unique_violation(&e) => return Err(ResolveError::IdentityConflict),
            Err(e) => return Err(e.into()),
        };

    promote_if_first_user(&state.auth, &id).await?;
    // LC-621: every user is a member of the General default enclave from the
    // moment they exist, not only after the next boot-time backfill. Best-effort
    // (the backfill is the safety net), so a transient failure here never 500s
    // the SSO callback.
    if let Ok(Some(general_id)) = db::enclave::get_general_id(&state.chat).await {
        if let Err(e) = db::enclave::ensure_general_membership(
            &state.chat,
            general_id,
            &id,
            crate::models::enclave::EnclaveRole::Member,
        )
        .await
        {
            tracing::warn!(target: "bunyip_sso", error = %e, user_id = %id, "General auto-join failed; backfill will retry");
        }
    }
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
    // LC-766: the provisioning sanitizer and the user-facing handle editor now
    // share one definition of a valid handle so a derived handle can never be
    // something the editor would later reject.
    crate::models::user::sanitize_handle(s)
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

// ── LC-587: suspicious-login approval ──────────────────────────────────────

const DEVICE_COOKIE: &str = "device_id";

#[derive(Debug, Deserialize)]
pub struct ApproveForm {
    pub token: String,
    pub code: String,
}

/// `POST /auth/bunyip/approve` - finish a login that was withheld as suspicious
/// (LC-587) by submitting the emailed 6-digit code. Public (no session yet),
/// mirroring the callback.
pub async fn post_approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<ApproveForm>,
) -> Response {
    let code = form.code.trim();
    match crate::login_approval::verify(&state.auth, &form.token, code).await {
        crate::login_approval::VerifyOutcome::Approved {
            user_id,
            country,
            device_hash,
        } => {
            let trust_proxy = crate::auth::proxy_headers_trusted(&state.settings).await;
            let (ua, ip) = crate::auth::extract_session_origin(&headers, trust_proxy);
            complete_login(
                &state,
                &jar,
                &user_id,
                ua.as_deref(),
                ip.as_deref(),
                country.as_deref(),
                device_hash,
            )
            .await
        }
        crate::login_approval::VerifyOutcome::Wrong => render_approval_page(
            &state,
            &form.token,
            Some("Incorrect code. Please try again."),
        ),
        // Expired / already used / too many attempts: send them back to restart
        // the sign-in from scratch.
        crate::login_approval::VerifyOutcome::Invalid => {
            Redirect::to("/login?sso_error=approval").into_response()
        }
    }
}

/// Read the first-party device id cookie, if the browser presents one.
fn read_device_id(jar: &CookieJar) -> Option<String> {
    jar.get(DEVICE_COOKIE)
        .map(|c| c.value().to_string())
        .filter(|s| !s.is_empty())
}

/// Long-lived first-party device cookie. `Lax` (not the session cookie's
/// `Strict`) so it is presented when the browser lands on the callback via the
/// top-level redirect back from Bunyip.
fn build_device_cookie(secure: bool, value: String) -> Cookie<'static> {
    let mut c = Cookie::new(DEVICE_COOKIE, value);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(Duration::days(365));
    c
}

/// Render the "approve this sign-in" interstitial for a pending challenge.
fn render_approval_page(state: &AppState, token: &str, error: Option<&str>) -> Response {
    let page = crate::views::login_approval::LoginApprovePage {
        asset_version: &state.asset_version,
        app_version: crate::version::VERSION,
        git_hash: crate::version::GIT_HASH,
        build_date: crate::version::BUILD_DATE,
        token,
        error,
    };
    match crate::views::html(&page) {
        Ok(html) => html.into_response(),
        Err(e) => {
            tracing::error!(target: "bunyip_sso", error = ?e, "approval page render failed");
            Redirect::to("/login?sso_error=internal").into_response()
        }
    }
}

/// Mint the session for a cleared/approved login, set the session cookie (plus a
/// device cookie when the browser had none), record the country/device
/// baseline, and redirect home. `device_hash` is `Some` when a device was
/// already identified (presented at login, or carried on the approved
/// challenge); otherwise a new device id is minted and its cookie set.
async fn complete_login(
    state: &AppState,
    jar: &CookieJar,
    user_id: &str,
    ua: Option<&str>,
    ip: Option<&str>,
    country: Option<&str>,
    device_hash: Option<String>,
) -> Response {
    let token = match db::auth::create_session_with_origin(&state.auth, user_id, ua, ip).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(target: "bunyip_sso", error = %e, "session create failed");
            return Redirect::to("/login?sso_error=internal").into_response();
        }
    };
    tracing::info!(target: "bunyip_sso", path = "sso", user_id = %user_id, "session created");
    let mut jar = jar
        .clone()
        .add(build_session_cookie(state.cookies_secure(), token));

    // Establish a device baseline. Use the already-known hash when present;
    // otherwise record the presented device, or mint + set one when the browser
    // sent none, so the next login from this browser is recognised.
    let device_hash = match device_hash {
        Some(h) => Some(h),
        None => match read_device_id(&jar) {
            Some(id) => Some(crate::login_approval::hash_device(&id)),
            None => {
                let id = new_random_token();
                let h = crate::login_approval::hash_device(&id);
                jar = jar.add(build_device_cookie(state.cookies_secure(), id));
                Some(h)
            }
        },
    };
    crate::login_approval::apply_baseline(
        &state.auth,
        user_id,
        country,
        device_hash.as_deref(),
        ua,
    )
    .await;
    (jar, Redirect::to("/")).into_response()
}

#[cfg(test)]
mod tests {
    //! LC-698: the resolver decides which local row an incoming Bunyip identity
    //! binds to, so its refusals are exercised here directly rather than through
    //! the full OIDC dance (which would need a live OP).
    use super::*;
    use crate::oidc::UserInfo;
    use crate::ws::hub::Hub;
    use sqlx::SqlitePool;
    use std::sync::Arc;

    async fn auth_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations/auth")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    async fn chat_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations/chat")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    async fn settings_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations/settings")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    async fn test_state() -> AppState {
        let auth = auth_pool().await;
        AppState {
            geoip: None,
            login_approval_enabled: false,
            bg: crate::bg::spawn(auth.clone()),
            auth,
            chat: chat_pool().await,
            settings: settings_pool().await,
            hub: Arc::new(Hub::new()),
            asset_version: "test".into(),
            last_seen_ledger: crate::auth::new_last_seen_ledger(),
            activity_ledger: crate::auth::new_last_seen_ledger(),
            secret_key: None,
            vapid: None,
            push_client: Arc::new(crate::push::MockPushClient::default()),
            apns_client: None,
            fcm_client: None,
            mailer: None,
            base_url: "http://localhost:8080".to_string(),
            ice_servers: "[]".to_string(),
            rate_limits: crate::rate_limit::RateLimits::new(),
            bunyip_sso: None,
            stt_client: None,
            llm_client: None,
            embedding_client: None,
        }
    }

    fn userinfo(sub: &str, email: &str, verified: bool) -> UserInfo {
        UserInfo {
            sub: sub.to_string(),
            email: Some(email.to_string()),
            email_verified: Some(verified),
            preferred_username: Some(email.split('@').next().unwrap().to_string()),
            name: None,
        }
    }

    /// A row linked to `sub` whose email is verified: the shape the resolver
    /// matches on when an incoming identity presents the same address.
    async fn linked_user(state: &AppState, username: &str, sub: &str, email: &str) -> String {
        let id = db::auth::create_user_from_bunyip(&state.auth, username, sub, None, Some(email))
            .await
            .unwrap();
        db::auth::mark_email_verified_if_unset(&state.auth, &id, email)
            .await
            .unwrap();
        id
    }

    /// The row's stored subject. Unlinked reads as `""` (the column is
    /// `NOT NULL DEFAULT ''`).
    async fn stored_sub(state: &AppState, user_id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT bunyip_sub FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&state.auth)
            .await
            .unwrap()
    }

    // LC-698 AC4: a rotated sub (the verified email matches a row linked to a
    // DIFFERENT subject) is an explicit identity conflict. Not a 500, and above
    // all not the silent relink LC-618 shipped.
    #[tokio::test]
    async fn rotated_sub_conflicts_instead_of_relinking() {
        let state = test_state().await;
        let alice = linked_user(&state, "alice", "sub-old", "alice@example.com").await;

        let err = resolve_or_provision_user(
            &state,
            "sub-new",
            &userinfo("sub-new", "alice@example.com", true),
        )
        .await
        .expect_err("a rotated sub must not resolve");
        assert!(matches!(err, ResolveError::IdentityConflict), "got {err:?}");
        assert_eq!(sso_error_code(&err), "identity_conflict");

        // The row is untouched: the incoming subject was never bound to it.
        assert_eq!(stored_sub(&state, &alice).await, "sub-old");
    }

    // LC-698 AC1 (the exploit from the issue): Mallory sets her profile email to
    // the new hire's address, unverified. The new hire's first login must not
    // land in her account.
    #[tokio::test]
    async fn self_service_email_never_adopts_another_row() {
        let state = test_state().await;
        let mallory = linked_user(&state, "mallory", "sub-mallory", "mallory@example.com").await;
        db::auth::set_user_email(&state.auth, &mallory, Some("newhire@example.com"))
            .await
            .unwrap();

        let id = resolve_or_provision_user(
            &state,
            "sub-newhire",
            &userinfo("sub-newhire", "newhire@example.com", true),
        )
        .await
        .expect("the new hire signs in");

        assert_ne!(id, mallory, "the login must not resolve to Mallory's row");
        assert_eq!(
            stored_sub(&state, &mallory).await,
            "sub-mallory",
            "Mallory's row keeps its own subject",
        );
        // The squatted address was released to its verified owner, so it no
        // longer routes Mallory's mail to the new hire either.
        assert_eq!(
            db::auth::get_user_email(&state.auth, &mallory)
                .await
                .unwrap(),
            None,
        );
        assert_eq!(
            db::auth::get_user_email(&state.auth, &id)
                .await
                .unwrap()
                .as_deref(),
            Some("newhire@example.com"),
        );
    }

    // LC-588 must not regress (AC2): an UNLINKED row with a verified email is
    // still adopted, so a pre-SSO account is claimed by its owner's first login.
    #[tokio::test]
    async fn unlinked_verified_row_is_still_adopted() {
        let state = test_state().await;
        let alice = linked_user(&state, "alice", "sub-old", "alice@example.com").await;
        // The admin unlink path (AC5) leaves exactly this state.
        assert!(db::auth::clear_bunyip_sub(&state.auth, &alice)
            .await
            .unwrap());

        let id = resolve_or_provision_user(
            &state,
            "sub-new",
            &userinfo("sub-new", "alice@example.com", true),
        )
        .await
        .expect("an unlinked row is adoptable");

        assert_eq!(id, alice, "the same row was adopted, not a duplicate");
        assert_eq!(stored_sub(&state, &alice).await, "sub-new");
    }

    // LC-698: an unverified assertion from the OP is not an adoption signal at
    // all, so the address stays with the row that holds it and the login fails
    // loudly on the email collision rather than binding to that row.
    #[tokio::test]
    async fn unverified_op_email_never_adopts() {
        let state = test_state().await;
        let alice = linked_user(&state, "alice", "sub-old", "alice@example.com").await;
        assert!(db::auth::clear_bunyip_sub(&state.auth, &alice)
            .await
            .unwrap());

        let err = resolve_or_provision_user(
            &state,
            "sub-new",
            &userinfo("sub-new", "alice@example.com", false),
        )
        .await
        .expect_err("an unverified OP email cannot claim a row");
        assert_eq!(sso_error_code(&err), "identity_conflict");
        assert_eq!(
            stored_sub(&state, &alice).await,
            "",
            "the row stays unlinked"
        );
    }
}
