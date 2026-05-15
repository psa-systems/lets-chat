use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};

use askama::Template;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::auth::VerifyEmailResultPage;
use crate::views::email_auth::{VerifyEmailHtml, VerifyEmailText};
use crate::views::{html, Html};

/// Best-effort dispatch of a verification email. Called from the register
/// route, the settings email-change handler, and the resend endpoint.
///
/// Failures are intentionally swallowed by the spawned task that wraps this
/// function: a transient SMTP outage must not block (or change the response
/// of) the request that triggered the send, because the user-visible
/// outcome is identical whether the message went out or not.
pub async fn dispatch_verification(
    state: &AppState,
    user_id: &str,
    email: &str,
) -> Result<(), String> {
    let token = db::email_verification::create_token(&state.auth, user_id, email)
        .await
        .map_err(|e| format!("create_token: {e}"))?;
    let link = format!("{}/verify-email/{}", state.base_url, token);
    let subject = "Verify your lets-chat email";
    let hours = db::email_verification::VERIFY_TTL_MINUTES / 60;
    let text = VerifyEmailText { link: &link, hours }
        .render()
        .map_err(|e| format!("render text: {e}"))?;
    let html_body = VerifyEmailHtml { link: &link, hours }
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

/// Fire-and-forget wrapper. Logs but never propagates dispatch errors. The
/// caller has already committed the user-visible state change (account
/// created, email updated, etc.) and the response must not depend on SMTP
/// being reachable.
pub fn spawn_dispatch(state: &AppState, user_id: String, email: String) {
    let s = state.clone();
    tokio::spawn(async move {
        if let Err(e) = dispatch_verification(&s, &user_id, &email).await {
            tracing::warn!(error = %e, "email verification dispatch failed");
        }
    });
}

/// GET /verify-email/{token} - consume the verification link. Returns a
/// small confirmation/error page regardless of whether the visitor is
/// logged in, so an emailed link works from any device.
pub async fn get_verify(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    if !state.mail_available() {
        return Err(AppError::NotFound);
    }

    let Some(active) = db::email_verification::find_active(&state.auth, &token).await? else {
        return Ok(render_result(
            &state,
            None,
            Some("This verification link is invalid or has expired."),
        ));
    };

    let updated =
        db::auth::mark_email_verified(&state.auth, &active.user_id, &active.email).await?;
    if updated == 0 {
        // Token was issued for a previous address; the user has since
        // changed (or removed) their email. Burn the token so a refresh
        // does not keep retrying.
        let _ = db::email_verification::mark_used(&state.auth, &token).await;
        return Ok(render_result(
            &state,
            None,
            Some("This link is no longer valid because the email on the account has changed."),
        ));
    }

    // mark_used returning 0 here means we lost a race against a parallel
    // verification of the same token. The account is still verified, so
    // showing the success page is the right outcome either way.
    let _ = db::email_verification::mark_used(&state.auth, &token).await?;
    db::email_verification::invalidate_all_for_user(&state.auth, &active.user_id).await?;

    Ok(render_result(
        &state,
        Some("Your email address is verified. Thanks!"),
        None,
    ))
}

/// POST /verify-email/resend - issue a fresh verification token for the
/// signed-in user's current email and dispatch it. Redirects back to
/// `/settings` with a flash query param. We don't expose finer-grained
/// outcomes (no email / already verified) because the settings page makes
/// those states visible already.
pub async fn post_resend(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    if !state.mail_available() {
        return Err(AppError::NotFound);
    }

    let email = db::auth::get_user_email(&state.auth, &user.id).await?;
    let verified = db::auth::get_user_email_verified_at(&state.auth, &user.id)
        .await?
        .is_some();

    if let Some(addr) = email {
        if !verified {
            // Burn any prior outstanding tokens before issuing a new one
            // so an attacker with the old link cannot beat the legitimate
            // user to the punch after a resend.
            db::email_verification::invalidate_all_for_user(&state.auth, &user.id).await?;
            spawn_dispatch(&state, user.id.clone(), addr);
        }
    }

    Ok(Redirect::to("/settings?verify_sent=1").into_response())
}

fn render_result(state: &AppState, notice: Option<&str>, error: Option<&str>) -> Response {
    let page = VerifyEmailResultPage {
        notice,
        error,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
    };
    let h: Result<Html, _> = html(&page);
    match h {
        Ok(h) => h.into_response(),
        Err(e) => AppError::Internal(format!("askama: {e}")).into_response(),
    }
}
