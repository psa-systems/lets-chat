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
        Err(ResolveError::IdentityConflict) => {
            tracing::warn!(target: "bunyip_sso", sub = %id_claims.sub, "sso reject: email already linked to a different identity");
            return Redirect::to("/login?sso_error=identity_conflict").into_response();
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

    // LC-627: bunyip is the authority for identity, so a bunyip-verified email
    // counts as verified here too. Provisioning never stamped `email_verified_at`,
    // which left every SSO user "unverified" to the features gated on it (notably
    // the remote-control consent gate, which then silently refused every request).
    // Stamp it now, idempotently, so new AND already-provisioned users are fixed on
    // their next login. Best-effort: a failure here must not block a valid login.
    if userinfo.email_verified == Some(true) {
        if let Err(e) = db::auth::mark_email_verified_if_unset(&state.auth, &user_id).await {
            tracing::warn!(target: "bunyip_sso", error = %e, user_id = %user_id, "email_verified stamp failed");
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
    /// LC-618: the verified email is already owned by a row this login cannot
    /// adopt (a bot row, an unverified-email match, or a banned row), so
    /// provisioning would collide on `UNIQUE(users.email)`. Surfaced to the user
    /// as an actionable message rather than an opaque internal error.
    IdentityConflict,
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

    // LC-588 + LC-618: the sub did not match above, but a users row may already
    // exist under the same VERIFIED email. LC-588 adopted an UNLINKED row (a
    // password user who signed up before SSO). LC-618 also re-adopts a row whose
    // bunyip_sub has ROTATED - the OP issued a NEW sub for the same verified
    // email (account recreated, or the OP's user store reseeded). Both cases link
    // this sub onto the existing row rather than INSERTing a duplicate that trips
    // UNIQUE(users.email) and 500s. Gated on email_verified so an unverified
    // address cannot claim an account; find_user_id_by_email excludes banned
    // rows; adopt_bunyip_sub refuses bot rows (returns false).
    if userinfo.email_verified == Some(true) {
        if let Some(email) = userinfo
            .email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if let Some(existing_id) = db::auth::find_user_id_by_email(&state.auth, email).await? {
                if db::auth::adopt_bunyip_sub(&state.auth, &existing_id, sub).await? {
                    return Ok(existing_id);
                }
                // The email is held by a bot row; refuse rather than provision a
                // colliding duplicate.
                return Err(ResolveError::IdentityConflict);
            }
        }
    }

    // Fresh sub with no adoptable verified-email row: provision. If an email is
    // already owned by a row we did not adopt above (an unverified-email match,
    // or a banned row that find_user_id_by_email excludes), the INSERT collides
    // on UNIQUE(users.email). Surface that as an actionable identity conflict
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
