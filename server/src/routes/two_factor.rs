use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use http::header::HeaderMap;
use http::HeaderValue;
use rand::Rng;
use serde::Deserialize;
use time::Duration;
use totp_rs::{Algorithm, Secret, TOTP};

use crate::auth::{AuthUser, SESSION_COOKIE};
use crate::crypto;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
#[cfg(feature = "standalone")]
use crate::views::two_factor::RegisterTwoFactorSetupPage;
use crate::views::two_factor::{
    LoginRecoveryPage, LoginTwoFactorPage, TwoFactorConfirmPage, TwoFactorSetupPage,
};
use crate::views::{html, Html};

pub const PENDING_COOKIE: &str = "pending_2fa";
#[cfg(feature = "standalone")]
pub const PENDING_REGISTRATION_COOKIE: &str = "pending_registration";
const TOTP_DIGITS: usize = 6;
const TOTP_STEP: u64 = 30;
const TOTP_SKEW: u8 = 1;
const ISSUER: &str = "lets-chat";
const RECOVERY_CODE_COUNT: usize = 8;
const RECOVERY_CHARSET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

#[derive(Deserialize)]
pub struct CodeForm {
    pub code: String,
}

pub async fn get_setup(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let Some(key) = state.secret_key.as_ref().map(|k| **k) else {
        return Err(AppError::NotFound);
    };
    if user.totp_enabled {
        return Err(AppError::Redirect(Redirect::to("/settings")));
    }

    let secret_bytes = generate_secret_bytes();
    let (encrypted, nonce) = crypto::seal(&key, &secret_bytes)
        .map_err(|e| AppError::Internal(format!("encrypt totp: {e}")))?;
    db::two_factor::set_totp_secret(&state.auth, &user.id, &encrypted, &nonce).await?;

    let totp = build_totp(secret_bytes, &user.username)?;
    let qr_base64 = totp
        .get_qr_base64()
        .map_err(|e| AppError::Internal(format!("qr: {e}")))?;
    let secret_b32 = totp.get_secret_base32();

    let page = TwoFactorSetupPage {
        username: &user.username,
        qr_base64: &qr_base64,
        secret_b32: &secret_b32,
        error: None,
        asset_version: &state.asset_version,
    };
    html(&page)
}

pub async fn post_setup(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<CodeForm>,
) -> Result<Response, AppError> {
    let Some(key) = state.secret_key.as_ref().map(|k| **k) else {
        return Err(AppError::NotFound);
    };
    let record = db::auth::find_user_by_id(&state.auth, &user.id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let (Some(encrypted), Some(nonce)) = (record.totp_secret_encrypted, record.totp_nonce) else {
        return Ok(Redirect::to("/settings/2fa/setup").into_response());
    };
    let secret_bytes = crypto::open(&key, &nonce, &encrypted)
        .map_err(|e| AppError::Internal(format!("decrypt totp: {e}")))?;

    let totp = build_totp(secret_bytes, &user.username)?;
    let supplied = form.code.trim();
    let ok = totp
        .check_current(supplied)
        .map_err(|e| AppError::Internal(format!("totp check: {e}")))?;
    if !ok {
        let qr_base64 = totp
            .get_qr_base64()
            .map_err(|e| AppError::Internal(format!("qr: {e}")))?;
        let secret_b32 = totp.get_secret_base32();
        let page = TwoFactorSetupPage {
            username: &user.username,
            qr_base64: &qr_base64,
            secret_b32: &secret_b32,
            error: Some("Invalid code. Try again."),
            asset_version: &state.asset_version,
        };
        return Ok((http::StatusCode::UNPROCESSABLE_ENTITY, html(&page)?).into_response());
    }

    let (plaintext_codes, hashes_json) = generate_recovery_codes()?;
    db::two_factor::enable_totp(&state.auth, &user.id, &hashes_json).await?;

    let page = TwoFactorConfirmPage {
        codes: &plaintext_codes,
        asset_version: &state.asset_version,
    };
    Ok(html(&page)?.into_response())
}

pub async fn get_login_challenge(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if !state.two_factor_available() {
        return Err(AppError::NotFound);
    }
    let Some(token) = jar.get(PENDING_COOKIE).map(|c| c.value().to_string()) else {
        return Ok(Redirect::to("/login").into_response());
    };
    if db::two_factor::get_pending_2fa_user(&state.auth, &token)
        .await?
        .is_none()
    {
        let jar = jar.remove(removal_cookie(PENDING_COOKIE));
        return Ok((jar, Redirect::to("/login")).into_response());
    }
    let page = LoginTwoFactorPage {
        error: None,
        asset_version: &state.asset_version,
    };
    Ok(html(&page)?.into_response())
}

pub async fn post_login_challenge(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<CodeForm>,
) -> Result<Response, AppError> {
    // LC-151: throttle TOTP guessing on the shared per-IP login budget.
    crate::routes::auth::enforce_login_rate_limit(&state, &headers).await?;
    let Some(key) = state.secret_key.as_ref().map(|k| **k) else {
        return Err(AppError::NotFound);
    };
    let Some(token) = jar.get(PENDING_COOKIE).map(|c| c.value().to_string()) else {
        return Ok(Redirect::to("/login").into_response());
    };
    let Some(user_id) = db::two_factor::get_pending_2fa_user(&state.auth, &token).await? else {
        let jar = jar.remove(removal_cookie(PENDING_COOKIE));
        return Ok((jar, Redirect::to("/login")).into_response());
    };

    let record = db::auth::find_user_by_id(&state.auth, &user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let (Some(encrypted), Some(nonce)) = (record.totp_secret_encrypted, record.totp_nonce) else {
        return Err(AppError::Internal("user has no totp secret".into()));
    };
    let secret_bytes = crypto::open(&key, &nonce, &encrypted)
        .map_err(|e| AppError::Internal(format!("decrypt totp: {e}")))?;
    let totp = build_totp(secret_bytes, &record.username)?;
    let ok = totp
        .check_current(form.code.trim())
        .map_err(|e| AppError::Internal(format!("totp check: {e}")))?;
    if !ok {
        let body = LoginTwoFactorPage {
            error: Some("Invalid code."),
            asset_version: &state.asset_version,
        }
        .render()?;
        return Ok((
            http::StatusCode::UNPROCESSABLE_ENTITY,
            axum::response::Html(body),
        )
            .into_response());
    }

    finalize_2fa_login(&state, &headers, jar, &token, &user_id).await
}

pub async fn get_login_recovery(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if !state.two_factor_available() {
        return Err(AppError::NotFound);
    }
    let Some(token) = jar.get(PENDING_COOKIE).map(|c| c.value().to_string()) else {
        return Ok(Redirect::to("/login").into_response());
    };
    if db::two_factor::get_pending_2fa_user(&state.auth, &token)
        .await?
        .is_none()
    {
        let jar = jar.remove(removal_cookie(PENDING_COOKIE));
        return Ok((jar, Redirect::to("/login")).into_response());
    }
    let page = LoginRecoveryPage {
        error: None,
        asset_version: &state.asset_version,
    };
    Ok(html(&page)?.into_response())
}

pub async fn post_login_recovery(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<CodeForm>,
) -> Result<Response, AppError> {
    // LC-151: throttle recovery-code guessing on the shared per-IP login budget.
    crate::routes::auth::enforce_login_rate_limit(&state, &headers).await?;
    if !state.two_factor_available() {
        return Err(AppError::NotFound);
    }
    let Some(token) = jar.get(PENDING_COOKIE).map(|c| c.value().to_string()) else {
        return Ok(Redirect::to("/login").into_response());
    };
    let Some(user_id) = db::two_factor::get_pending_2fa_user(&state.auth, &token).await? else {
        let jar = jar.remove(removal_cookie(PENDING_COOKIE));
        return Ok((jar, Redirect::to("/login")).into_response());
    };
    let record = db::auth::find_user_by_id(&state.auth, &user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let supplied = form.code.trim().to_uppercase();
    let hashes_json = record.totp_recovery_hashes.unwrap_or_else(|| "[]".into());
    match consume_recovery_code(&hashes_json, &supplied)? {
        Some(remaining_json) => {
            db::two_factor::update_recovery_hashes(&state.auth, &user_id, &remaining_json).await?;
            finalize_2fa_login(&state, &headers, jar, &token, &user_id).await
        }
        None => {
            let body = LoginRecoveryPage {
                error: Some("Invalid recovery code."),
                asset_version: &state.asset_version,
            }
            .render()?;
            Ok((
                http::StatusCode::UNPROCESSABLE_ENTITY,
                axum::response::Html(body),
            )
                .into_response())
        }
    }
}

async fn finalize_2fa_login(
    state: &AppState,
    headers: &HeaderMap,
    jar: CookieJar,
    pending_token: &str,
    user_id: &str,
) -> Result<Response, AppError> {
    let (ua, ip) = crate::auth::extract_session_origin(headers);
    let session =
        db::auth::create_session_with_origin(&state.auth, user_id, ua.as_deref(), ip.as_deref())
            .await?;
    crate::routes::login_alerts::spawn_dispatch(state, user_id.to_string(), ua, ip);
    let _ = db::two_factor::delete_pending_2fa(&state.auth, pending_token).await;

    let session_cookie = build_session_cookie(state.cookies_secure(), session);
    let jar = jar
        .add(session_cookie)
        .remove(removal_cookie(PENDING_COOKIE));

    if is_htmx(headers) {
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

pub fn build_pending_cookie(secure: bool, token: String) -> Cookie<'static> {
    let mut c = Cookie::new(PENDING_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::minutes(5));
    c
}

fn build_session_cookie(secure: bool, token: String) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::days(30));
    c
}

fn removal_cookie(name: &str) -> Cookie<'static> {
    let mut c = Cookie::new(name.to_string(), "");
    c.set_path("/");
    c.make_removal();
    c
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

fn generate_secret_bytes() -> Vec<u8> {
    Secret::generate_secret().to_bytes().expect("secret bytes")
}

pub fn build_totp(secret_bytes: Vec<u8>, username: &str) -> Result<TOTP, AppError> {
    TOTP::new(
        Algorithm::SHA1,
        TOTP_DIGITS,
        TOTP_SKEW,
        TOTP_STEP,
        secret_bytes,
        Some(ISSUER.to_string()),
        username.to_string(),
    )
    .map_err(|e| AppError::Internal(format!("totp: {e}")))
}

fn generate_recovery_codes() -> Result<(Vec<String>, String), AppError> {
    let mut rng = rand::thread_rng();
    let mut plaintext: Vec<String> = Vec::with_capacity(RECOVERY_CODE_COUNT);
    let mut hashes: Vec<String> = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let mut chars = String::with_capacity(9);
        for i in 0..8 {
            if i == 4 {
                chars.push('-');
            }
            let idx = rng.gen_range(0..RECOVERY_CHARSET.len());
            chars.push(RECOVERY_CHARSET[idx] as char);
        }
        let hash = hash_recovery(&chars)?;
        plaintext.push(chars);
        hashes.push(hash);
    }
    let json =
        serde_json::to_string(&hashes).map_err(|e| AppError::Internal(format!("json: {e}")))?;
    Ok((plaintext, json))
}

fn hash_recovery(code: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2
        .hash_password(code.as_bytes(), &salt)
        .map_err(|e| AppError::Internal(format!("argon2: {e}")))?
        .to_string())
}

/// On match, returns the remaining-hashes JSON with the matched entry removed.
/// Stash a partially-validated registration so the user must complete TOTP
/// enrollment before the account is actually created. Generates the TOTP
/// secret, encrypts it with the server key, writes a `pending_registrations`
/// row, sets a short-lived cookie and redirects to `/register/2fa`.
#[cfg(feature = "standalone")]
pub async fn stash_pending_registration(
    state: &AppState,
    headers: &HeaderMap,
    jar: CookieJar,
    username: &str,
    email: Option<&str>,
    password_hash: &str,
) -> Result<Response, AppError> {
    let Some(key) = state.secret_key.as_ref().map(|k| **k) else {
        return Err(AppError::Internal(
            "two_factor_available without secret_key".into(),
        ));
    };
    let secret_bytes = generate_secret_bytes();
    let (encrypted, nonce) = crypto::seal(&key, &secret_bytes)
        .map_err(|e| AppError::Internal(format!("encrypt totp: {e}")))?;
    let token = db::two_factor::create_pending_registration(
        &state.auth,
        username,
        email,
        password_hash,
        &encrypted,
        &nonce,
    )
    .await?;

    let cookie = build_pending_registration_cookie(state.cookies_secure(), token);
    let jar = jar.add(cookie);

    if is_htmx(headers) {
        let mut resp = Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap();
        resp.headers_mut()
            .insert("HX-Redirect", HeaderValue::from_static("/register/2fa"));
        return Ok((jar, resp).into_response());
    }
    Ok((jar, Redirect::to("/register/2fa")).into_response())
}

#[cfg(feature = "standalone")]
pub async fn get_register_2fa(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let Some(key) = state.secret_key.as_ref().map(|k| **k) else {
        return Err(AppError::NotFound);
    };
    let Some(token) = jar
        .get(PENDING_REGISTRATION_COOKIE)
        .map(|c| c.value().to_string())
    else {
        return Ok(Redirect::to("/register").into_response());
    };
    let Some(pending) = db::two_factor::get_pending_registration(&state.auth, &token).await? else {
        let jar = jar.remove(removal_cookie(PENDING_REGISTRATION_COOKIE));
        return Ok((jar, Redirect::to("/register")).into_response());
    };

    let secret_bytes = crypto::open(&key, &pending.totp_nonce, &pending.totp_secret_encrypted)
        .map_err(|e| AppError::Internal(format!("decrypt totp: {e}")))?;
    let totp = build_totp(secret_bytes, &pending.username)?;
    let qr_base64 = totp
        .get_qr_base64()
        .map_err(|e| AppError::Internal(format!("qr: {e}")))?;
    let secret_b32 = totp.get_secret_base32();
    let page = RegisterTwoFactorSetupPage {
        username: &pending.username,
        qr_base64: &qr_base64,
        secret_b32: &secret_b32,
        error: None,
        asset_version: &state.asset_version,
    };
    Ok(html(&page)?.into_response())
}

#[cfg(feature = "standalone")]
pub async fn post_register_2fa(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<CodeForm>,
) -> Result<Response, AppError> {
    let Some(key) = state.secret_key.as_ref().map(|k| **k) else {
        return Err(AppError::NotFound);
    };
    let Some(token) = jar
        .get(PENDING_REGISTRATION_COOKIE)
        .map(|c| c.value().to_string())
    else {
        return Ok(Redirect::to("/register").into_response());
    };
    let Some(pending) = db::two_factor::get_pending_registration(&state.auth, &token).await? else {
        let jar = jar.remove(removal_cookie(PENDING_REGISTRATION_COOKIE));
        return Ok((jar, Redirect::to("/register")).into_response());
    };

    let secret_bytes = crypto::open(&key, &pending.totp_nonce, &pending.totp_secret_encrypted)
        .map_err(|e| AppError::Internal(format!("decrypt totp: {e}")))?;
    let totp = build_totp(secret_bytes, &pending.username)?;
    let ok = totp
        .check_current(form.code.trim())
        .map_err(|e| AppError::Internal(format!("totp check: {e}")))?;
    if !ok {
        return render_register_setup_error(&state, &pending, &totp, "Invalid code. Try again.");
    }

    // Code verified - materialize the user atomically with the TOTP fields
    // and recovery codes. Username/email uniqueness is re-checked here since
    // someone else may have claimed them between stash and confirm.
    let user_id =
        match db::auth::create_user(&state.auth, &pending.username, &pending.password_hash).await {
            Ok(id) => id,
            Err(e) => {
                if crate::routes::auth::is_unique_violation(&e) {
                    let _ = db::two_factor::delete_pending_registration(&state.auth, &token).await;
                    let jar = jar.remove(removal_cookie(PENDING_REGISTRATION_COOKIE));
                    return Ok((jar, Redirect::to("/register?taken=1")).into_response());
                }
                return Err(AppError::Internal(format!("register: {e}")));
            }
        };

    db::two_factor::set_totp_secret(
        &state.auth,
        &user_id,
        &pending.totp_secret_encrypted,
        &pending.totp_nonce,
    )
    .await?;
    let (plaintext_codes, hashes_json) = generate_recovery_codes()?;
    db::two_factor::enable_totp(&state.auth, &user_id, &hashes_json).await?;

    if let Some(ref e) = pending.email {
        if let Err(err) = db::auth::set_user_email(&state.auth, &user_id, Some(e)).await {
            if crate::routes::auth::is_unique_violation(&err) {
                // Roll back so the username does not get reserved by a
                // half-finished registration whose email collided.
                let _ = db::auth::delete_user(&state.auth, &user_id).await;
                let _ = db::two_factor::delete_pending_registration(&state.auth, &token).await;
                let jar = jar.remove(removal_cookie(PENDING_REGISTRATION_COOKIE));
                return Ok((jar, Redirect::to("/register?email_taken=1")).into_response());
            }
            return Err(AppError::Internal(format!("set_user_email: {err}")));
        }
        if state.mail_available() {
            crate::routes::email_verification::spawn_dispatch(&state, user_id.clone(), e.clone());
        }
    }

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
    let session =
        db::auth::create_session_with_origin(&state.auth, &user_id, ua.as_deref(), ip.as_deref())
            .await?;
    let _ = db::login_alerts::check_and_record_device(
        &state.auth,
        &user_id,
        ua.as_deref(),
        ip.as_deref(),
    )
    .await;
    let _ = db::two_factor::delete_pending_registration(&state.auth, &token).await;

    let session_cookie = build_session_cookie(state.cookies_secure(), session);
    let jar = jar
        .add(session_cookie)
        .remove(removal_cookie(PENDING_REGISTRATION_COOKIE));

    let page = TwoFactorConfirmPage {
        codes: &plaintext_codes,
        asset_version: &state.asset_version,
    };
    Ok((jar, html(&page)?).into_response())
}

#[cfg(feature = "standalone")]
fn render_register_setup_error(
    state: &AppState,
    pending: &db::two_factor::PendingRegistration,
    totp: &TOTP,
    msg: &'static str,
) -> Result<Response, AppError> {
    let qr_base64 = totp
        .get_qr_base64()
        .map_err(|e| AppError::Internal(format!("qr: {e}")))?;
    let secret_b32 = totp.get_secret_base32();
    let page = RegisterTwoFactorSetupPage {
        username: &pending.username,
        qr_base64: &qr_base64,
        secret_b32: &secret_b32,
        error: Some(msg),
        asset_version: &state.asset_version,
    };
    Ok((http::StatusCode::UNPROCESSABLE_ENTITY, html(&page)?).into_response())
}

#[cfg(feature = "standalone")]
async fn promote_first_user_to_admin(state: &AppState, user_id: &str) -> Result<bool, AppError> {
    let mut tx = state.auth.begin().await?;
    // LC-73: bots excluded so they cannot consume the first-user-is-admin slot.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_bot = 0")
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
pub fn build_pending_registration_cookie(secure: bool, token: String) -> Cookie<'static> {
    let mut c = Cookie::new(PENDING_REGISTRATION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::minutes(30));
    c
}

fn consume_recovery_code(hashes_json: &str, supplied: &str) -> Result<Option<String>, AppError> {
    let mut hashes: Vec<String> =
        serde_json::from_str(hashes_json).map_err(|e| AppError::Internal(format!("json: {e}")))?;
    let argon2 = Argon2::default();
    let mut hit: Option<usize> = None;
    for (i, h) in hashes.iter().enumerate() {
        let Ok(parsed) = PasswordHash::new(h) else {
            continue;
        };
        if argon2.verify_password(supplied.as_bytes(), &parsed).is_ok() {
            hit = Some(i);
            break;
        }
    }
    let Some(i) = hit else {
        return Ok(None);
    };
    hashes.remove(i);
    let json =
        serde_json::to_string(&hashes).map_err(|e| AppError::Internal(format!("json: {e}")))?;
    Ok(Some(json))
}
