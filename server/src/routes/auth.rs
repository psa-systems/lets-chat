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
use serde::Deserialize;
use time::Duration;

use crate::auth::SESSION_COOKIE;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::auth::{FormErrors, LoginPage, RegisterPage};
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub password: String,
    pub password_confirm: String,
}

pub async fn get_login(State(state): State<AppState>) -> Result<Html, AppError> {
    let page = LoginPage {
        error: None,
        asset_version: &state.asset_version,
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
        let cookie = crate::routes::two_factor::build_pending_cookie(pending);
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

    let token = db::auth::create_session(&state.auth, &record.id).await?;
    let cookie = build_session_cookie(token);
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

pub async fn get_register(State(state): State<AppState>) -> Result<Html, AppError> {
    let page = RegisterPage {
        error: None,
        asset_version: &state.asset_version,
    };
    html(&page)
}

pub async fn post_register(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    axum::Form(form): axum::Form<RegisterForm>,
) -> Result<Response, AppError> {
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

    let password_hash = match hash_password(password) {
        Ok(h) => h,
        Err(e) => return Err(AppError::Internal(format!("hash: {}", e))),
    };

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
            return Err(AppError::Internal(format!("register: {}", e)));
        }
    };

    let promoted = promote_first_user_to_admin(&state, &user_id).await?;

    if promoted {
        if let Err(e) = db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
            tracing::warn!(error = %e, "enclave backfill after first registration failed");
        }
    }

    let token = db::auth::create_session(&state.auth, &user_id).await?;
    let cookie = build_session_cookie(token);
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

fn build_session_cookie(token: String) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::days(30));
    c
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

enum FormPage {
    Login,
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
            }
            .render(),
            FormPage::Register => RegisterPage {
                error: Some(msg),
                asset_version: &state.asset_version,
            }
            .render(),
        }
    };
    let body = match body {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "failed to render form_error template");
            return AppError::Internal(format!("askama: {}", e)).into_response();
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

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.is_unique_violation())
}
