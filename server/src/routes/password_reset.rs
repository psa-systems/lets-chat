use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
use argon2::Argon2;
use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use http::header::HeaderMap;
use http::HeaderValue;
use serde::Deserialize;

use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::auth::{ForgotPage, FormErrors, ResetPage};
use crate::views::email_auth::{PasswordResetHtml, PasswordResetText};
use crate::views::{html, Html};

/// Always-200 placeholder shown after the user submits `/forgot`. We never
/// distinguish between "address unknown" and "email sent" so that the page
/// cannot be used to enumerate registered users.
const FORGOT_GENERIC_NOTICE: &str =
    "If that address matches an account, we've sent a link to reset your password. Check your inbox.";

#[derive(Deserialize)]
pub struct ForgotForm {
    pub email: String,
}

#[derive(Deserialize)]
pub struct ResetForm {
    pub password: String,
    pub password_confirm: String,
}

/// Render the "enter your email" form. Returns 404 when SMTP is not
/// configured so admins running without a mailer don't expose a broken flow.
pub async fn get_forgot(State(state): State<AppState>) -> Result<Response, AppError> {
    if state.local_login_disabled || !state.mail_available() {
        return Err(AppError::NotFound);
    }
    let page = ForgotPage {
        error: None,
        notice: None,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    let h: Html = html(&page)?;
    Ok(h.into_response())
}

/// Accept an email, look up the account, generate a single-use reset token,
/// and dispatch the email. Always returns 200 with the same notice text to
/// avoid leaking whether the address is registered.
pub async fn post_forgot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ForgotForm>,
) -> Result<Response, AppError> {
    if state.local_login_disabled || !state.mail_available() {
        return Err(AppError::NotFound);
    }
    let email = form.email.trim().to_string();
    if email.is_empty() {
        return Ok(forgot_error(&state, &headers, "Email is required"));
    }

    // Generate + send happens off the response path: a failure to talk to
    // the SMTP server must not block the user-visible response, both because
    // the response cannot reveal account existence and because retrying the
    // request would issue a second token.
    let state_for_task = state.clone();
    let email_for_task = email.clone();
    tokio::spawn(async move {
        if let Err(e) = dispatch_reset(&state_for_task, &email_for_task).await {
            tracing::warn!(error = %e, "password reset dispatch failed");
        }
    });

    Ok(forgot_notice(&state, &headers, FORGOT_GENERIC_NOTICE))
}

async fn dispatch_reset(state: &AppState, email: &str) -> Result<(), String> {
    let Some(user_id) = db::auth::find_user_id_by_email(&state.auth, email)
        .await
        .map_err(|e| format!("lookup: {e}"))?
    else {
        // Unknown address - do nothing. Same 200 was already returned.
        return Ok(());
    };
    let token = db::password_reset::create_token(&state.auth, &user_id)
        .await
        .map_err(|e| format!("create_token: {e}"))?;
    let link = format!("{}/reset/{}", state.base_url, token);
    let subject = "Reset your lets-chat password";
    let ttl = db::password_reset::RESET_TTL_MINUTES;
    let text = PasswordResetText { link: &link, ttl }
        .render()
        .map_err(|e| format!("render text: {e}"))?;
    let html_body = PasswordResetHtml { link: &link, ttl }
        .render()
        .map_err(|e| format!("render html: {e}"))?;
    let mailer = state
        .mailer
        .as_ref()
        .ok_or_else(|| "mailer not configured".to_string())?;
    mailer
        .send_multipart(email, subject, &text, &html_body)
        .await
        .map_err(|e| format!("send: {e}"))?;
    Ok(())
}

/// Render the "choose a new password" form. The token is re-validated on
/// submit, so we don't strictly need to validate here - but rejecting an
/// expired or unknown link up front is friendlier than letting the user
/// type a new password and then bouncing them.
pub async fn get_reset(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    if state.local_login_disabled || !state.mail_available() {
        return Err(AppError::NotFound);
    }
    let active = db::password_reset::find_active_user_id(&state.auth, &token).await?;
    if active.is_none() {
        return Err(AppError::NotFound);
    }
    let page = ResetPage {
        token: &token,
        error: None,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    let h: Html = html(&page)?;
    Ok(h.into_response())
}

pub async fn post_reset(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ResetForm>,
) -> Result<Response, AppError> {
    if state.local_login_disabled || !state.mail_available() {
        return Err(AppError::NotFound);
    }
    if form.password.len() < 8 {
        return Ok(reset_error(
            &state,
            &headers,
            &token,
            "Password must be at least 8 characters",
        ));
    }
    if form.password != form.password_confirm {
        return Ok(reset_error(
            &state,
            &headers,
            &token,
            "Passwords do not match",
        ));
    }

    let Some(user_id) = db::password_reset::find_active_user_id(&state.auth, &token).await? else {
        // Don't render the form again - the link is dead. 404 surfaces the
        // expired/used/unknown case unambiguously.
        return Err(AppError::NotFound);
    };

    let password_hash =
        hash_password(&form.password).map_err(|e| AppError::Internal(format!("hash: {e}")))?;
    db::auth::set_password_hash(&state.auth, &user_id, &password_hash).await?;

    // Atomically mark this token as used so it cannot be replayed, and burn
    // every other pending token for this user so a parallel inbox link stops
    // working too.
    let consumed = db::password_reset::mark_used(&state.auth, &token).await?;
    if consumed == 0 {
        // Lost a race against another reset for the same token (or it
        // expired between the SELECT above and now). Bail without making
        // the password change visible to the user.
        return Err(AppError::NotFound);
    }
    db::password_reset::invalidate_all_for_user(&state.auth, &user_id).await?;

    // Force-sign-out every existing session so an attacker who had already
    // hijacked one cannot keep using it.
    db::auth::delete_user_sessions(&state.auth, &user_id).await?;

    if is_htmx(&headers) {
        let mut resp = Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap();
        resp.headers_mut()
            .insert("HX-Redirect", HeaderValue::from_static("/login"));
        Ok(resp)
    } else {
        Ok(Redirect::to("/login").into_response())
    }
}

fn forgot_error(state: &AppState, headers: &HeaderMap, msg: &str) -> Response {
    if is_htmx(headers) {
        return render_form_errors(Some(msg), None);
    }
    let page = ForgotPage {
        error: Some(msg),
        notice: None,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    render_full_page(page, http::StatusCode::UNPROCESSABLE_ENTITY)
}

fn forgot_notice(state: &AppState, headers: &HeaderMap, msg: &str) -> Response {
    if is_htmx(headers) {
        return render_form_errors(None, Some(msg));
    }
    let page = ForgotPage {
        error: None,
        notice: Some(msg),
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    render_full_page(page, http::StatusCode::OK)
}

fn reset_error(state: &AppState, headers: &HeaderMap, token: &str, msg: &str) -> Response {
    if is_htmx(headers) {
        return render_form_errors(Some(msg), None);
    }
    let page = ResetPage {
        token,
        error: Some(msg),
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    render_full_page(page, http::StatusCode::UNPROCESSABLE_ENTITY)
}

fn render_form_errors(error: Option<&str>, notice: Option<&str>) -> Response {
    // The forgot template renders both notice (green) and error (red) inside
    // the same #form-errors target, so we emit a tiny ad-hoc fragment that
    // covers both. Reset's form-errors fragment only ever shows an error,
    // and the FormErrors view already covers that case.
    let body = if let Some(n) = notice {
        format!("<p class=\"text-green-700 text-sm\">{}</p>", html_escape(n))
    } else {
        match (FormErrors { error }).render() {
            Ok(s) => s,
            Err(e) => return AppError::Internal(format!("askama: {e}")).into_response(),
        }
    };
    (http::StatusCode::OK, axum::response::Html(body)).into_response()
}

fn render_full_page<T: Template>(page: T, status: http::StatusCode) -> Response {
    match page.render() {
        Ok(body) => (status, axum::response::Html(body)).into_response(),
        Err(e) => AppError::Internal(format!("askama: {e}")).into_response(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}
