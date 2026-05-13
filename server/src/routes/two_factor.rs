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
use crate::views::two_factor::{
    LoginRecoveryPage, LoginTwoFactorPage, TwoFactorConfirmPage, TwoFactorSetupPage,
};
use crate::views::{html, Html};

pub const PENDING_COOKIE: &str = "pending_2fa";
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

    let session_cookie = build_session_cookie(session);
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

pub fn build_pending_cookie(token: String) -> Cookie<'static> {
    let mut c = Cookie::new(PENDING_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::minutes(5));
    c
}

fn build_session_cookie(token: String) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(true);
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
