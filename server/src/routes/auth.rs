#[cfg(feature = "standalone")]
use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
use argon2::password_hash::{PasswordHash, PasswordVerifier};
use argon2::Argon2;
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use http::header::HeaderMap;
use http::HeaderValue;
use serde::Deserialize;
use time::Duration;

use crate::auth::SESSION_COOKIE;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
#[cfg(feature = "standalone")]
use crate::views::auth::RegisterPage;
use crate::views::auth::{FormErrors, LoginPage};
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

#[cfg(feature = "standalone")]
#[derive(Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub password: String,
    pub password_confirm: String,
    #[serde(default)]
    pub email: Option<String>,
    /// LC-94 honeypot. The register form renders a hidden input with
    /// this name, off-screen and `autocomplete=off`; legit users
    /// never see it and never fill it in. Bots that auto-fill every
    /// input populate the field, and the handler rejects the
    /// submission. Wrapped in `Option` so old clients without the
    /// field still deserialize cleanly.
    #[serde(default)]
    pub website: Option<String>,
}

pub async fn get_login(State(state): State<AppState>) -> Result<Html, AppError> {
    let page = LoginPage {
        error: None,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    html(&page)
}

pub async fn post_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<LoginForm>,
) -> Result<Response, AppError> {
    let record = match db::auth::find_user_by_username(&state.auth, &form.username).await? {
        Some(r) if !r.is_banned => r,
        _ => {
            return Ok(form_error(
                &state,
                &headers,
                FormPage::Login,
                "Invalid username or password",
            ));
        }
    };

    if !verify_password(&record.password_hash, &form.password) {
        return Ok(form_error(
            &state,
            &headers,
            FormPage::Login,
            "Invalid username or password",
        ));
    }

    if record.totp_enabled && state.two_factor_available() {
        let pending = db::two_factor::create_pending_2fa(&state.auth, &record.id).await?;
        let cookie =
            crate::routes::two_factor::build_pending_cookie(state.cookies_secure(), pending);
        let jar = jar.add(cookie);
        if is_htmx(&headers) {
            let mut resp = Response::builder()
                .status(200)
                .body(axum::body::Body::empty())
                .unwrap();
            resp.headers_mut()
                .insert("HX-Redirect", HeaderValue::from_static("/login/2fa"));
            return Ok((jar, resp).into_response());
        }
        return Ok((jar, Redirect::to("/login/2fa")).into_response());
    }

    let (ua, ip) = crate::auth::extract_session_origin(&headers);
    let token =
        db::auth::create_session_with_origin(&state.auth, &record.id, ua.as_deref(), ip.as_deref())
            .await?;
    crate::routes::login_alerts::spawn_dispatch(&state, record.id.clone(), ua, ip);
    let cookie = build_session_cookie(state.cookies_secure(), token);
    let jar = jar.add(cookie);

    if is_htmx(&headers) {
        let mut resp = Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap();
        resp.headers_mut()
            .insert("HX-Redirect", HeaderValue::from_static("/"));
        Ok((jar, resp).into_response())
    } else {
        Ok((jar, Redirect::to("/")).into_response())
    }
}

#[cfg(feature = "standalone")]
pub async fn get_register(State(state): State<AppState>) -> Result<Html, AppError> {
    let page = RegisterPage {
        error: None,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    html(&page)
}

#[cfg(feature = "standalone")]
pub async fn post_register(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<RegisterForm>,
) -> Result<Response, AppError> {
    // LC-94: honeypot. If the hidden field is populated the request
    // came from a bot. Return a generic-looking error so the bot
    // cannot tell it was caught; do not surface a distinct message
    // (which would let the spammer A/B around the field).
    let honeypot_on = db::settings::get_setting(&state.settings, "honeypot_enabled")
        .await?
        .as_deref()
        == Some("true");
    if honeypot_on
        && form
            .website
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    {
        return Ok(form_error(
            &state,
            &headers,
            FormPage::Register,
            "Could not create account; please try again",
        ));
    }

    // LC-94: per-IP rate limit. Skipped when the cap is 0 (default),
    // when the request has no extractable client IP (no reverse
    // proxy upstream), or when the deployment-wide honeypot is the
    // only defense the operator wants. Each rate-limited 429 still
    // surfaces a clear error so legitimate retries know to wait.
    let reg_cap =
        crate::rate_limit::read_u32_setting(&state.settings, "rate_limit_registrations").await;
    if let Some(ip) = crate::rate_limit::client_ip_for_rate_limit(&state.settings, &headers).await {
        if let crate::rate_limit::Outcome::Deny { retry_after } =
            state
                .rate_limits
                .check(crate::rate_limit::RateLimitKind::Register, &ip, reg_cap)
        {
            return Err(AppError::TooManyRequests(
                format!("too many registrations from this address; retry in {retry_after} seconds"),
                retry_after,
            ));
        }
    }

    let username = form.username.trim();
    let password = form.password.as_str();
    if username.len() < 3 || username.len() > 32 {
        return Ok(form_error(
            &state,
            &headers,
            FormPage::Register,
            "Username must be 3-32 characters",
        ));
    }
    if password.len() < 8 {
        return Ok(form_error(
            &state,
            &headers,
            FormPage::Register,
            "Password must be at least 8 characters",
        ));
    }
    if password != form.password_confirm.as_str() {
        return Ok(form_error(
            &state,
            &headers,
            FormPage::Register,
            "Passwords do not match",
        ));
    }

    let email = form
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(ref e) = email {
        if e.chars().count() > 254 || !looks_like_email_register(e) {
            return Ok(form_error(
                &state,
                &headers,
                FormPage::Register,
                "Email address is not valid",
            ));
        }
    }

    let password_hash = match hash_password(password) {
        Ok(h) => h,
        Err(e) => return Err(AppError::Internal(format!("hash: {e}"))),
    };

    // When 2FA enforcement is on for this deployment, defer the actual user
    // creation until after the TOTP code is verified. Otherwise an abandoned
    // setup leaves a fully-registered account behind that squats the
    // username and (if SMTP is wired) has already fired a verification
    // email - both surprising for the operator and the prospective user.
    if state.two_factor_available() {
        return crate::routes::two_factor::stash_pending_registration(
            &state,
            &headers,
            jar,
            username,
            email.as_deref(),
            &password_hash,
        )
        .await;
    }

    let user_id = match db::auth::create_user(&state.auth, username, &password_hash).await {
        Ok(id) => id,
        Err(e) => {
            if is_unique_violation(&e) {
                return Ok(form_error(
                    &state,
                    &headers,
                    FormPage::Register,
                    "Username taken",
                ));
            }
            return Err(AppError::Internal(format!("register: {e}")));
        }
    };

    if let Some(ref e) = email {
        if let Err(err) = db::auth::set_user_email(&state.auth, &user_id, Some(e)).await {
            if is_unique_violation(&err) {
                // Roll back the freshly created user so the username does not
                // get squatted by a registration attempt that ultimately
                // failed validation.
                let _ = db::auth::delete_user(&state.auth, &user_id).await;
                return Ok(form_error(
                    &state,
                    &headers,
                    FormPage::Register,
                    "That email address is already in use",
                ));
            }
            return Err(AppError::Internal(format!("set_user_email: {err}")));
        }

        // Kick off a verification email when SMTP is configured. The send
        // is fire-and-forget: registration must succeed even if the relay
        // is unreachable, and the user can resend from settings later.
        if state.mail_available() {
            crate::routes::email_verification::spawn_dispatch(&state, user_id.clone(), e.clone());
        }
    }

    // Apply the admin-configured default for new users. The migration
    // defaults `notify_email_digest_enabled` to 0; this hook overrides
    // it when the operator has flipped `default_notify_email_digest`.
    // Failure to apply is logged but not fatal: the worst case is the
    // user has to opt in manually.
    if let Ok(Some(ref v)) =
        db::settings::get_setting(&state.settings, "default_notify_email_digest").await
    {
        if v == "1" {
            if let Err(e) =
                db::auth::set_notify_email_digest_enabled(&state.auth, &user_id, true).await
            {
                tracing::warn!(error = %e, user_id = %user_id, "default digest opt-in apply failed");
            }
        }
    }

    let promoted = promote_first_user_to_admin(&state, &user_id).await?;

    if promoted {
        if let Err(e) = db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
            tracing::warn!(error = %e, "enclave backfill after first registration failed");
        }
    }

    let (ua, ip) = crate::auth::extract_session_origin(&headers);
    let token =
        db::auth::create_session_with_origin(&state.auth, &user_id, ua.as_deref(), ip.as_deref())
            .await?;
    // Pre-seed the device fingerprint so the first login from the same
    // browser does not fire a "new device" alert. The user just created
    // the account from this device - alerting them about it is noise.
    let _ = db::login_alerts::check_and_record_device(
        &state.auth,
        &user_id,
        ua.as_deref(),
        ip.as_deref(),
    )
    .await;
    let cookie = build_session_cookie(state.cookies_secure(), token);
    let jar = jar.add(cookie);

    if is_htmx(&headers) {
        let mut resp = Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap();
        resp.headers_mut()
            .insert("HX-Redirect", HeaderValue::from_static("/"));
        Ok((jar, resp).into_response())
    } else {
        Ok((jar, Redirect::to("/")).into_response())
    }
}

pub async fn get_logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        let _ = db::auth::delete_session(&state.auth, c.value()).await;
    }
    let mut clear = Cookie::new(SESSION_COOKIE, "");
    clear.set_path("/");
    clear.make_removal();
    let jar = jar.remove(clear);
    Ok((jar, Redirect::to("/login")).into_response())
}

pub(super) fn build_session_cookie(secure: bool, token: String) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::days(30));
    c
}

pub(super) fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

enum FormPage {
    Login,
    #[cfg(feature = "standalone")]
    Register,
}

fn form_error(state: &AppState, headers: &HeaderMap, page: FormPage, msg: &str) -> Response {
    let body = if is_htmx(headers) {
        FormErrors { error: Some(msg) }.render()
    } else {
        match page {
            FormPage::Login => LoginPage {
                error: Some(msg),
                asset_version: &state.asset_version,
                app_version: version::VERSION,
                git_hash: version::GIT_HASH,
                build_date: version::BUILD_DATE,
            }
            .render(),
            #[cfg(feature = "standalone")]
            FormPage::Register => RegisterPage {
                error: Some(msg),
                asset_version: &state.asset_version,
                app_version: version::VERSION,
                git_hash: version::GIT_HASH,
                build_date: version::BUILD_DATE,
            }
            .render(),
        }
    };
    let body = match body {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "failed to render form_error template");
            return AppError::Internal(format!("askama: {e}")).into_response();
        }
    };
    (
        http::StatusCode::UNPROCESSABLE_ENTITY,
        axum::response::Html(body),
    )
        .into_response()
}

/// Promote a freshly created user to admin if and only if they are the first
/// user in the system. The check + update is wrapped in a single transaction
/// to avoid a race where two concurrent registrations both observe a count
/// past 1 and neither bootstraps the admin.
#[cfg(feature = "standalone")]
async fn promote_first_user_to_admin(state: &AppState, user_id: &str) -> Result<bool, AppError> {
    let mut tx = state.auth.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    let promoted = count == 1;
    if promoted {
        if let Err(e) = sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await
        {
            tracing::error!(error = %e, user_id, "failed to promote first user to admin");
            return Err(AppError::Internal(format!("promote admin: {e}")));
        }
    }
    tx.commit().await?;
    Ok(promoted)
}

#[cfg(feature = "standalone")]
fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

fn verify_password(hash: &str, password: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(feature = "standalone")]
pub(super) fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.is_unique_violation())
}

/// Mirrors `settings::looks_like_email`. Duplicated here to keep the
/// standalone auth flow free of cross-module dependencies on the settings
/// route. Loose syntactic check, not RFC validation.
#[cfg(feature = "standalone")]
fn looks_like_email_register(s: &str) -> bool {
    if s.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let mut parts = s.split('@');
    let local = match parts.next() {
        Some(l) if !l.is_empty() => l,
        _ => return false,
    };
    let domain = match parts.next() {
        Some(d) if !d.is_empty() => d,
        _ => return false,
    };
    if parts.next().is_some() {
        return false;
    }
    let _ = local;
    domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}
