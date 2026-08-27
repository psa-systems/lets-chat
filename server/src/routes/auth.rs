//! LC-22 pure-RP cutover: the auth surface is the SSO shell + logout.
//!
//! The local username + password handlers are gone. `routes/bunyip_sso.rs`
//! mints the session via `db::auth::create_session_with_origin` +
//! `build_session_cookie` (the same helpers the deleted `post_login` used).

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use time::Duration;

use crate::auth::SESSION_COOKIE;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::auth::LoginPage;
use crate::views::{html, Html};

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    pub sso_error: Option<String>,
}

pub async fn get_login(
    State(state): State<AppState>,
    Query(q): Query<LoginQuery>,
) -> Result<Html, AppError> {
    let sso_error_msg = q.sso_error.as_deref().map(map_sso_error);
    let host = state
        .bunyip_sso
        .as_ref()
        .map(|c| c.config.issuer.host_str().unwrap_or("Bunyip").to_string())
        .unwrap_or_else(|| "Bunyip".to_string());
    let page = build_login_page(&state, sso_error_msg, &host).await?;
    html(&page)
}

fn map_sso_error(code: &str) -> &'static str {
    match code {
        "dance" => "Sign-in could not be completed. Please try again.",
        "op" => "Bunyip rejected the sign-in attempt.",
        "banned" => "Your account is suspended.",
        "identity_conflict" => {
            "This account's email is already linked to a different sign-in identity. Contact an administrator."
        }
        "internal" => "An internal error occurred. Please try again.",
        // LC-826: the development-only LETS_CHAT_DEV_NO_SSO opt-out booted with
        // no RP at all.
        "unconfigured" => "Single sign-on is not configured on this server.",
        _ => "Sign-in failed.",
    }
}

/// Resolve global branding and bake it into a `LoginPage`. Now mostly cosmetic
/// chrome around the SSO button.
pub(crate) async fn build_login_page<'a>(
    state: &'a AppState,
    sso_error: Option<&'a str>,
    bunyip_issuer_host: &'a str,
) -> Result<LoginPage<'a>, AppError> {
    let branding = db::branding::resolve(&state.chat, db::branding::Scope::Global).await?;
    let brand_body_html = if branding.login_body.trim().is_empty() {
        String::new()
    } else {
        crate::views::markdown::render_login_body(&branding.login_body)
    };
    Ok(LoginPage {
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        build_date: version::BUILD_DATE,
        brand_logo: branding.logo_upload_id.is_some(),
        brand_heading: branding.login_heading,
        brand_body_html,
        sso_error,
        bunyip_issuer_host,
    })
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
    // LC-472: land on the public marketing page (LC-470) after logout, not the
    // bare SSO /login shell. The landing carries the "Sign in with Bunyip" CTA.
    Ok((jar, Redirect::to("/")).into_response())
}

#[cfg_attr(not(feature = "standalone"), allow(dead_code))]
pub(crate) fn build_session_cookie(secure: bool, token: String) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, token);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::days(30));
    c
}
