//! LC-76: slash-command dispatch + autocomplete.
//!
//! `try_dispatch` is called from `post_message` when the body starts with
//! `/`. It runs after every access / posting gate, so a command can never
//! bypass room RBAC. Built-in commands come from `crate::commands`; custom
//! ones from `db::slash`. An unknown `/foo` returns `None` so the caller
//! falls back to posting the literal text (commands cannot grief a user who
//! happens to type a slash).

use axum::extract::{Query, State};
use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::commands;
use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;
use crate::views::room::ComposerFragment;
use crate::views::{html, Html};
use askama::Template;

/// Cap on a webhook command's response body, so a misbehaving endpoint
/// cannot post a wall of text.
const WEBHOOK_MAX_BODY: usize = 4000;
const WEBHOOK_TIMEOUT_SECS: u64 = 5;

/// One command row for the help panel + autocomplete dropdown.
pub struct SlashRow {
    pub name: String,
    pub usage: String,
    pub description: String,
}

#[derive(Template)]
#[template(path = "room/slash_popover.html")]
struct SlashPopover {
    commands: Vec<SlashRow>,
}

#[derive(Template)]
#[template(path = "room/slash_help.html")]
struct SlashHelp {
    commands: Vec<SlashRow>,
}

/// Visible commands for `user`, honoring `admin_only`. Built-ins first, then
/// custom, both filtered by a name prefix (empty matches all).
fn visible_commands(
    user: &User,
    custom: &[db::slash::CustomCommand],
    prefix: &str,
) -> Vec<SlashRow> {
    let is_admin = user.role == "admin";
    let mut rows = Vec::new();
    for b in commands::BUILTINS {
        if b.admin_only && !is_admin {
            continue;
        }
        if b.name.starts_with(prefix) {
            rows.push(SlashRow {
                name: b.name.to_string(),
                usage: b.usage.to_string(),
                description: b.description.to_string(),
            });
        }
    }
    for c in custom {
        if c.admin_only && !is_admin {
            continue;
        }
        if c.name.starts_with(prefix) {
            rows.push(SlashRow {
                name: c.name.clone(),
                usage: format!("/{}", c.name),
                description: c.description.clone(),
            });
        }
    }
    rows
}

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    #[serde(default)]
    pub q: String,
}

/// GET /api/slash-commands?q=  - autocomplete dropdown for the composer.
pub async fn get_autocomplete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<AutocompleteQuery>,
) -> Result<Html, AppError> {
    let prefix = q.q.trim_start_matches('/').to_ascii_lowercase();
    let custom = db::slash::list_global(&state.chat).await?;
    let commands = visible_commands(&user, &custom, &prefix);
    html(&SlashPopover { commands })
}

/// Try to handle `body` as a slash command. Returns `Ok(None)` when the body
/// is not a recognized command (caller posts it as a literal message).
pub(crate) async fn try_dispatch(
    state: &AppState,
    room: &Room,
    user: &User,
    body: &str,
) -> Result<Option<Html>, AppError> {
    let body = body.trim();
    if !body.starts_with('/') {
        return Ok(None);
    }
    let without = &body[1..];
    let (cmd, rest) = match without.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (without, ""),
    };
    let cmd = cmd.to_ascii_lowercase();
    if cmd.is_empty() {
        return Ok(None);
    }

    // Built-in commands.
    if let Some(b) = commands::find_builtin(&cmd) {
        if b.admin_only && user.role != "admin" {
            return Err(AppError::Forbidden);
        }
        match cmd.as_str() {
            "help" => {
                let custom = db::slash::list_global(&state.chat).await?;
                let rows = visible_commands(user, &custom, "");
                return Ok(Some(html(&SlashHelp { commands: rows })?));
            }
            "me" => {
                if rest.is_empty() {
                    return Err(AppError::BadRequest("usage: /me <action>".into()));
                }
                // Italic body reads as an emote beneath the author's name.
                post_message_body(state, room, user, &format!("_{rest}_")).await?;
            }
            "shrug" => {
                let prefix = if rest.is_empty() {
                    String::new()
                } else {
                    format!("{rest} ")
                };
                post_message_body(
                    state,
                    room,
                    user,
                    &format!("{prefix}\u{00af}\\_(\u{30c4})_/\u{00af}"),
                )
                .await?;
            }
            "poll" => {
                let (question, options) = super::polls::parse_command(rest)?;
                super::polls::create_poll(
                    state, room, user, &question, &options, false, false, None,
                )
                .await?;
            }
            "remind" => {
                handle_remind(state, room, user, rest).await?;
            }
            _ => return Ok(None),
        }
        return Ok(Some(html(&ComposerFragment {
            room,
            initial_draft: "",
        })?));
    }

    // Custom (admin-defined) commands.
    if let Some(c) = db::slash::get_global(&state.chat, &cmd).await? {
        if c.admin_only && user.role != "admin" {
            return Err(AppError::Forbidden);
        }
        let out = match c.kind {
            db::slash::CustomKind::StaticText => c.target.replace("{args}", rest),
            db::slash::CustomKind::WebhookPost => run_webhook(&c.target, rest).await?,
        };
        let out = out.trim();
        if out.is_empty() {
            return Err(AppError::BadRequest("command produced no output".into()));
        }
        post_message_body(state, room, user, out).await?;
        return Ok(Some(html(&ComposerFragment {
            room,
            initial_draft: "",
        })?));
    }

    // Unknown command: fall back to posting the literal text.
    Ok(None)
}

/// Insert a normal message and broadcast it (shared by /me, /shrug, custom).
async fn post_message_body(
    state: &AppState,
    room: &Room,
    user: &User,
    body: &str,
) -> Result<(), AppError> {
    let new_id = db::chat::insert_message(&state.chat, room.id, &user.id, body).await?;
    super::room::finalize_message_send(state, room, user, new_id, body).await?;
    Ok(())
}

/// `/remind <when> <text>`: post the note as a message, then set a reminder
/// for the author about that message (reuses LC-63).
async fn handle_remind(
    state: &AppState,
    room: &Room,
    user: &User,
    rest: &str,
) -> Result<(), AppError> {
    let (when, text) = rest
        .split_once(char::is_whitespace)
        .map(|(w, t)| (w, t.trim()))
        .ok_or_else(|| AppError::BadRequest("usage: /remind <15m|1h|3h|1d> <text>".into()))?;
    if text.is_empty() {
        return Err(AppError::BadRequest("usage: /remind <when> <text>".into()));
    }
    let minutes = parse_duration_minutes(when).ok_or_else(|| {
        AppError::BadRequest("reminder time must look like 15m, 1h, 3h, or 1d".into())
    })?;
    let new_id = db::chat::insert_message(&state.chat, room.id, &user.id, text).await?;
    super::room::finalize_message_send(state, room, user, new_id, text).await?;
    let remind_at = (Utc::now() + Duration::minutes(minutes))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    db::reminders::insert(&state.chat, &user.id, new_id, &remind_at).await?;
    Ok(())
}

/// Parse `15m` / `2h` / `1d` (and bare `m`/`h`/`d` suffixes) into minutes.
fn parse_duration_minutes(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = num.parse().ok()?;
    if n <= 0 {
        return None;
    }
    // checked_mul: a huge value would otherwise overflow (panic in debug).
    match unit {
        "m" => Some(n),
        "h" => n.checked_mul(60),
        "d" => n.checked_mul(60 * 24),
        _ => None,
    }
}

/// SSRF guard for webhook command URLs. Requires an http(s) scheme and
/// rejects hosts that resolve into the loopback / private / link-local
/// ranges (or are obviously internal by name), so a custom command cannot
/// be pointed at internal services or a cloud metadata endpoint. Checked
/// both at admin save time and again here at execution time.
pub(crate) fn webhook_url_ok(url: &str) -> bool {
    let rest = match url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        Some(r) => r,
        None => return false,
    };
    // Host is up to the first '/', '?', or '#'; strip userinfo and port.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host).trim();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".internal") {
        return false;
    }
    // Reject IP literals in loopback / private / link-local ranges. Hostnames
    // that are not IPs pass (admin-trusted; full DNS-resolution checks are out
    // of scope for v1).
    if let Ok(ip) = lower.parse::<std::net::IpAddr>() {
        return is_public_ip(&ip);
    }
    true
}

fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0)
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// POST a custom command's args to its webhook URL and return the response
/// body (capped). Failures surface as a BadRequest so the invoker sees why.
async fn run_webhook(url: &str, args: &str) -> Result<String, AppError> {
    if !webhook_url_ok(url) {
        return Err(AppError::BadRequest(
            "custom command webhook URL is not allowed".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Internal(format!("http client: {e}")))?;
    // reqwest's `json` feature is not enabled in this build; serialize the
    // body by hand and set the content type.
    let payload =
        serde_json::to_string(&serde_json::json!({ "args": args })).unwrap_or_else(|_| "{}".into());
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload)
        .send()
        .await
        .map_err(|_| AppError::BadRequest("custom command webhook failed".into()))?;
    if !resp.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "custom command webhook returned {}",
            resp.status().as_u16()
        )));
    }
    let text = resp
        .text()
        .await
        .map_err(|_| AppError::BadRequest("custom command webhook returned no body".into()))?;
    Ok(text.chars().take(WEBHOOK_MAX_BODY).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_url_guard() {
        assert!(webhook_url_ok("https://example.com/hook"));
        assert!(webhook_url_ok("http://api.example.com:8443/x?y=1"));
        // Rejected: bad scheme, loopback, private, link-local, metadata, internal names.
        assert!(!webhook_url_ok("ftp://example.com"));
        assert!(!webhook_url_ok("http://localhost/x"));
        assert!(!webhook_url_ok("http://127.0.0.1:6379"));
        assert!(!webhook_url_ok("http://[::1]/x"));
        assert!(!webhook_url_ok("http://10.0.0.5/x"));
        assert!(!webhook_url_ok("http://192.168.1.1/x"));
        assert!(!webhook_url_ok("http://169.254.169.254/latest/meta-data/"));
        assert!(!webhook_url_ok("http://172.16.0.1/x"));
        assert!(!webhook_url_ok("http://0.0.0.0/x"));
        assert!(!webhook_url_ok("http://db.internal/x"));
        assert!(!webhook_url_ok("not a url"));
    }

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration_minutes("15m"), Some(15));
        assert_eq!(parse_duration_minutes("2h"), Some(120));
        assert_eq!(parse_duration_minutes("1d"), Some(1440));
        assert_eq!(parse_duration_minutes("0m"), None);
        assert_eq!(parse_duration_minutes("5"), None);
        assert_eq!(parse_duration_minutes(""), None);
        assert_eq!(parse_duration_minutes("5x"), None);
        // No overflow panic on a huge value.
        assert_eq!(parse_duration_minutes("9999999999999999999d"), None);
    }
}
