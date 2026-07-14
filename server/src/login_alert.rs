//! LC-580: new-login-location alert.
//!
//! On a genuine login (spliced into the Bunyip SSO callback after the session
//! is created), resolve the client IP to a country and, when it differs from
//! the country recorded at the user's previous login, email a "new sign-in
//! from <country>" alert. Reuses the orphaned `login_alert` email scaffold
//! (templates + view structs + the per-user `notify_login_alerts_enabled`
//! opt-out) left behind by the removed local-auth path. Entirely best-effort:
//! every failure is logged and swallowed so it can never affect the login, and
//! the whole thing no-ops when no IP2Location DB is configured.

use std::net::IpAddr;

use askama::Template;

use crate::db;
use crate::geoip::LocationDecision;
use crate::state::AppState;
use crate::views::login_alert::{LoginAlertHtml, LoginAlertText};

/// Entry point spliced into the SSO callback. `ip` / `ua` are the
/// trusted-proxy-aware origin already captured for the session row.
pub async fn maybe_alert(state: &AppState, user_id: &str, ip: Option<&str>, ua: Option<&str>) {
    // Kill-switch: no IP2Location DB configured.
    let Some(geoip) = state.geoip.as_ref() else {
        return;
    };
    // A resolvable public client IP is required.
    let Some(parsed) = ip.and_then(|s| s.parse::<IpAddr>().ok()) else {
        return;
    };
    if !crate::geoip::is_public_ip(&parsed) {
        return;
    }
    let Some(country) = geoip.country_code(parsed) else {
        return;
    };

    let last = match db::auth::get_last_login_country(&state.auth, user_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "login_alert", user_id, error = %e, "get_last_login_country failed; skipping");
            return;
        }
    };

    match crate::geoip::location_decision(last.as_deref(), &country) {
        LocationDecision::Unchanged => {}
        // First geolocatable login: record the country silently, no alert.
        LocationDecision::Record => {
            persist_country(state, user_id, &country).await;
        }
        // Country changed: email the alert (gated + best-effort), then persist
        // the new country so a repeat login from it does not re-alert.
        LocationDecision::Alert => {
            send_alert(state, user_id, &country, parsed, ua).await;
            persist_country(state, user_id, &country).await;
        }
    }
}

async fn persist_country(state: &AppState, user_id: &str, country: &str) {
    if let Err(e) = db::auth::set_last_login_country(&state.auth, user_id, country).await {
        tracing::warn!(target: "login_alert", user_id, error = %e, "set_last_login_country failed");
    }
}

/// Render + send the new-login-location email, gated exactly like the other
/// transactional notifications (recipient present + verified + opted in +
/// mailer configured). Any failure is logged, never propagated.
async fn send_alert(state: &AppState, user_id: &str, country: &str, ip: IpAddr, ua: Option<&str>) {
    let user = match db::auth::find_user_by_id(&state.auth, user_id).await {
        Ok(Some(u)) => u,
        _ => return,
    };
    let Some(email) = user.email.as_deref().filter(|s| !s.is_empty()) else {
        return;
    };
    // Respect the per-user opt-out (reused scaffold column).
    if !user.notify_login_alerts_enabled {
        return;
    }
    // Only email a verified address: an unverified one may not be the user's.
    let verified = matches!(
        db::auth::get_user_email_verified_at(&state.auth, user_id).await,
        Ok(Some(_))
    );
    if !verified {
        return;
    }
    let Some(mailer) = state.mailer.as_ref() else {
        return;
    };

    let ip_str = ip.to_string();
    let when = chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string();
    let device_label = ua.unwrap_or("unknown");
    let base = state.base_url.trim_end_matches('/');
    let sessions_url = format!("{base}/settings");
    let settings_url = sessions_url.clone();

    let text = LoginAlertText {
        username: &user.username,
        country,
        device_label,
        ip: &ip_str,
        when: &when,
        sessions_url: &sessions_url,
        settings_url: &settings_url,
    };
    let html = LoginAlertHtml {
        username: &user.username,
        country,
        device_label,
        ip: &ip_str,
        when: &when,
        sessions_url: &sessions_url,
        settings_url: &settings_url,
    };
    let (Ok(text_body), Ok(html_body)) = (text.render(), html.render()) else {
        tracing::warn!(target: "login_alert", user_id, "login_alert template render failed");
        return;
    };
    let subject = "New sign-in to your Let's Chat account";
    if let Err(e) = mailer
        .send_multipart(email, subject, &text_body, &html_body)
        .await
    {
        tracing::warn!(target: "login_alert", user_id, error = %e, "new-login-location email failed");
    }
}
