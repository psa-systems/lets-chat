use askama::Template;

use crate::db;
use crate::state::AppState;
use crate::views::login_alert::{LoginAlertHtml, LoginAlertText};

/// Best-effort dispatch: if this `(user_agent, ip)` pair has never been seen
/// for `user_id` and the user has login alerts enabled and an email on file,
/// send a notification. Errors are logged; the caller has already issued
/// the session and cannot wait for SMTP.
async fn dispatch_login_alert(
    state: &AppState,
    user_id: &str,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<(), String> {
    // Always record the device first, even when mail is unavailable or the
    // user has alerts disabled. Otherwise an operator enabling SMTP later
    // would treat every existing user's every existing browser as "new" and
    // generate a noisy backfill on each one's next sign-in.
    let is_new = db::login_alerts::check_and_record_device(&state.auth, user_id, user_agent, ip)
        .await
        .map_err(|e| format!("check_and_record_device: {e}"))?;
    if !is_new {
        tracing::debug!(user_id, "login alert: known device, no email");
        return Ok(());
    }
    // From here on the device IS new. Every early-return path is a
    // suppression an operator might want to diagnose ("why didn't my
    // user get an email?"), so each one emits a log line that names the
    // exact reason. The dispatch is fire-and-forget, so this is the
    // only signal an operator gets.
    if !state.mail_available() {
        tracing::debug!(
            user_id,
            "login alert suppressed: SMTP not configured (mail_available=false)"
        );
        return Ok(());
    }
    let record = match db::auth::find_user_by_id(&state.auth, user_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            tracing::warn!(
                user_id,
                "login alert: user row vanished between session create and dispatch"
            );
            return Ok(());
        }
        Err(e) => return Err(format!("find_user_by_id: {e}")),
    };
    if !record.notify_login_alerts_enabled {
        tracing::debug!(
            user_id,
            username = %record.username,
            "login alert suppressed: user has notify_login_alerts_enabled = 0"
        );
        return Ok(());
    }
    let Some(email) = record.email.as_deref().filter(|e| !e.is_empty()) else {
        tracing::debug!(
            user_id,
            username = %record.username,
            "login alert suppressed: no email address on account"
        );
        return Ok(());
    };

    let device_label = super::settings::summarize_user_agent(user_agent);
    let ip_str = ip.unwrap_or("");
    let when = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let sessions_url = format!("{}/settings#sessions", state.base_url);
    let settings_url = format!("{}/settings", state.base_url);

    let text = LoginAlertText {
        username: &record.username,
        device_label: &device_label,
        ip: ip_str,
        when: &when,
        sessions_url: &sessions_url,
        settings_url: &settings_url,
    }
    .render()
    .map_err(|e| format!("render text: {e}"))?;
    let html = LoginAlertHtml {
        username: &record.username,
        device_label: &device_label,
        ip: ip_str,
        when: &when,
        sessions_url: &sessions_url,
        settings_url: &settings_url,
    }
    .render()
    .map_err(|e| format!("render html: {e}"))?;

    let mailer = state
        .mailer
        .as_ref()
        .ok_or_else(|| "mailer not configured".to_string())?;
    mailer
        .send_multipart(email, "New sign-in to your lets-chat account", &text, &html)
        .await
        .map_err(|e| format!("send: {e}"))?;
    tracing::debug!(
        user_id,
        username = %record.username,
        device = %device_label,
        "login alert sent"
    );
    Ok(())
}

/// Fire-and-forget wrapper. Logs but never propagates errors. The caller
/// has already minted a session and the user is on their way; SMTP latency
/// or failure must not block (or change) the login response.
pub fn spawn_dispatch(
    state: &AppState,
    user_id: String,
    user_agent: Option<String>,
    ip: Option<String>,
) {
    let s = state.clone();
    tokio::spawn(async move {
        if let Err(e) =
            dispatch_login_alert(&s, &user_id, user_agent.as_deref(), ip.as_deref()).await
        {
            tracing::warn!(error = %e, "login alert dispatch failed");
        }
    });
}
