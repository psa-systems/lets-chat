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

/// Map an `sso_error` code to its message-catalog key. The template applies
/// `|t`, so the banner follows the request locale.
fn map_sso_error(code: &str) -> &'static str {
    match code {
        "dance" => "login-sso-error-dance",
        "op" => "login-sso-error-op",
        "banned" => "login-sso-error-banned",
        "identity_conflict" => "login-sso-error-identity-conflict",
        "internal" => "login-sso-error-internal",
        // LC-826: the development-only LETS_CHAT_DEV_NO_SSO opt-out booted with
        // no RP at all.
        "unconfigured" => "login-sso-error-unconfigured",
        // LC-939: the emailed approval code (LC-587) expired, was already used,
        // or hit the attempt cap. Distinct from a generic SSO failure because the
        // recovery is just signing in again.
        "approval" => "login-sso-error-approval",
        _ => GENERIC_SSO_ERROR,
    }
}

const GENERIC_SSO_ERROR: &str = "login-sso-error-generic";

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
        environment: crate::environment::deployment_environment(),
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

#[cfg(test)]
mod tests {
    use super::{map_sso_error, GENERIC_SSO_ERROR};
    use regex::Regex;
    use std::collections::HashSet;

    // LC-939: `bunyip_sso.rs` redirects to `/login?sso_error=<code>` from
    // several call sites via a string literal, not through `sso_error_code`,
    // so a new one can silently take `map_sso_error`'s catch-all (as happened
    // with "approval"). Enumerate every literal in that file and require a
    // non-default arm here for each, so the next one fails this test instead.
    #[test]
    fn every_literal_sso_error_code_has_a_mapped_message() {
        let source = include_str!("bunyip_sso.rs");
        let re = Regex::new(r#"sso_error=([A-Za-z_]+)"#).unwrap();
        let codes: HashSet<&str> = re
            .captures_iter(source)
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        assert!(!codes.is_empty(), "expected to find sso_error literals");

        for code in codes {
            assert_ne!(
                map_sso_error(code),
                GENERIC_SSO_ERROR,
                "sso_error={code} falls through to the generic catch-all"
            );
        }
    }
}
