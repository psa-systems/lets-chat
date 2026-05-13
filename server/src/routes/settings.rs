use axum::extract::{Multipart, Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::auth::{AuthUser, CurrentSessionId, SESSION_COOKIE};
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::settings::{BlockedListPage, BlockedUserView, SessionView, UserSettingsPage};
use crate::views::{html, Html};

const MAX_AVATAR_BYTES: usize = 1024 * 1024;
const MAX_DISPLAY_NAME_CHARS: usize = 64;
const MAX_BIO_CHARS: usize = 500;
const MAX_EMAIL_CHARS: usize = 254;
#[cfg(feature = "standalone")]
const MIN_PASSWORD_CHARS: usize = 8;

#[derive(Deserialize, Default)]
pub struct SettingsQuery {
    #[serde(default)]
    pub password_changed: Option<String>,
    #[serde(default)]
    pub password_error: Option<String>,
    #[serde(default)]
    pub verify_sent: Option<String>,
    #[serde(default)]
    pub session_revoked: Option<String>,
}

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(default)]
    pub read_receipts_enabled: Option<String>,
    #[serde(default)]
    pub is_profile_public: Option<String>,
    #[serde(default)]
    pub notify_browser_enabled: Option<String>,
    #[serde(default)]
    pub notify_sound_enabled: Option<String>,
    #[serde(default)]
    pub notify_push_enabled: Option<String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<SettingsQuery>,
    jar: CookieJar,
) -> Result<Html, AppError> {
    tracing::info!("get_settings enter");
    let chrome_start = std::time::Instant::now();
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    tracing::info!(
        elapsed_ms = chrome_start.elapsed().as_millis() as u64,
        "get_settings: load_chrome done"
    );
    let email = db::auth::get_user_email(&state.auth, &user.id).await?;
    tracing::info!("get_settings: email done");
    let email_verified = db::auth::get_user_email_verified_at(&state.auth, &user.id)
        .await?
        .is_some();
    tracing::info!("get_settings: email_verified done");
    let email_verification_available = cfg!(feature = "standalone") && state.mail_available();
    let password_error = q.password_error.as_deref().and_then(password_error_message);
    let current_session = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());
    let sessions = build_session_views(&state, &user.id, current_session.as_deref()).await?;
    tracing::info!("get_settings: sessions done");
    let page = UserSettingsPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        saved: false,
        push_available: state.push_available(),
        email,
        email_verified,
        email_verification_available,
        email_verify_sent: q.verify_sent.is_some(),
        password_change_available: cfg!(feature = "standalone"),
        password_changed: q.password_changed.is_some(),
        password_error,
        sessions: &sessions,
        session_revoked: q.session_revoked.is_some(),
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
    };
    let render_start = std::time::Instant::now();
    let result = html(&page);
    tracing::info!(
        elapsed_ms = render_start.elapsed().as_millis() as u64,
        "get_settings: html render done"
    );
    result
}

async fn build_session_views(
    state: &AppState,
    user_id: &str,
    current_session: Option<&str>,
) -> Result<Vec<SessionView>, AppError> {
    let rows = db::auth::list_sessions_for_user(&state.auth, user_id).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let label = summarize_user_agent(r.user_agent.as_deref());
            let last_seen = r.last_seen_at.unwrap_or_else(|| r.created_at.clone());
            SessionView {
                is_current: current_session == Some(r.id.as_str()),
                id: r.id,
                label,
                ip: r.ip,
                last_seen,
                created: r.created_at,
            }
        })
        .collect())
}

/// Reduce a User-Agent string to a "{browser} on {os}" label. Worst case
/// (unrecognized UA), we fall back to a generic "Unknown device" string;
/// the row still has its IP + first-seen timestamp so it remains
/// identifiable. We deliberately do not parse with a heavyweight UA
/// database: the rough family is enough to let the user spot a session
/// they did not create.
fn summarize_user_agent(ua: Option<&str>) -> String {
    let Some(ua) = ua else {
        return "Unknown device".to_string();
    };
    let lower = ua.to_ascii_lowercase();
    let os = detect_os(&lower);
    let browser = if lower.contains("edg/") {
        "Edge"
    } else if lower.contains("opr/") || lower.contains("opera") {
        "Opera"
    } else if lower.contains("firefox") {
        "Firefox"
    } else if lower.contains("chrome") && !lower.contains("chromium") {
        "Chrome"
    } else if lower.contains("chromium") {
        "Chromium"
    } else if lower.contains("safari") {
        "Safari"
    } else if lower.contains("lets-chat-desktop") {
        "Let's Chat desktop"
    } else {
        "Browser"
    };
    format!("{browser} on {os}")
}

/// OS detection. Order matters: Android UAs contain "Linux", and iPadOS in
/// "desktop mode" advertises "Mac OS X". Check the more specific tokens
/// first, then fall through to general families.
fn detect_os(lower: &str) -> String {
    // ChromeOS first - its UA looks like "X11; CrOS x86_64 14541.0.0" with
    // no "linux" token, so a naive Linux check misses it entirely.
    if lower.contains("cros") {
        return "ChromeOS".to_string();
    }
    // iPadOS in desktop-site mode reports "Macintosh; Intel Mac OS X" but
    // keeps the "ipad" token in some builds; check Apple-mobile tokens
    // before macOS so we do not relabel iPads as Macs.
    if lower.contains("iphone") {
        return version_after(lower, "iphone os ")
            .or_else(|| version_after(lower, "os "))
            .map(|v| format!("iOS {}", normalize_apple_version(&v)))
            .unwrap_or_else(|| "iOS".to_string());
    }
    if lower.contains("ipad") {
        return version_after(lower, "os ")
            .map(|v| format!("iPadOS {}", normalize_apple_version(&v)))
            .unwrap_or_else(|| "iPadOS".to_string());
    }
    // Android before Linux: Android UAs include "Linux; Android 14; ..."
    if lower.contains("android") {
        return version_after(lower, "android ")
            .map(|v| format!("Android {v}"))
            .unwrap_or_else(|| "Android".to_string());
    }
    if lower.contains("windows nt") {
        // Microsoft did not bump the NT version for Windows 11, so the UA
        // cannot distinguish 10 from 11. Label both as "Windows 10/11" to
        // avoid lying to the user.
        return match version_after(lower, "windows nt ").as_deref() {
            Some("10.0") => "Windows 10/11".to_string(),
            Some("6.3") => "Windows 8.1".to_string(),
            Some("6.2") => "Windows 8".to_string(),
            Some("6.1") => "Windows 7".to_string(),
            Some(v) => format!("Windows NT {v}"),
            None => "Windows".to_string(),
        };
    }
    if lower.contains("windows") {
        return "Windows".to_string();
    }
    if lower.contains("mac os x") || lower.contains("macintosh") {
        return version_after(lower, "mac os x ")
            .map(|v| format!("macOS {}", normalize_apple_version(&v)))
            .unwrap_or_else(|| "macOS".to_string());
    }
    if lower.contains("freebsd") {
        return "FreeBSD".to_string();
    }
    if lower.contains("openbsd") {
        return "OpenBSD".to_string();
    }
    if lower.contains("netbsd") {
        return "NetBSD".to_string();
    }
    if lower.contains("fuchsia") {
        return "Fuchsia".to_string();
    }
    if lower.contains("linux") {
        return "Linux".to_string();
    }
    if lower.contains("x11") || lower.contains("unix") {
        return "Unix".to_string();
    }
    "Unknown OS".to_string()
}

/// Return the version token that immediately follows `prefix` in `s`. The
/// token runs until the next character that is not a digit, `.`, or `_`
/// (Apple uses underscores). Returns `None` if the prefix is missing or
/// the following character is not a digit.
fn version_after(s: &str, prefix: &str) -> Option<String> {
    let idx = s.find(prefix)?;
    let tail = &s[idx + prefix.len()..];
    let end = tail
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '_'))
        .unwrap_or(tail.len());
    let token = &tail[..end];
    if token.chars().next()?.is_ascii_digit() {
        Some(token.to_string())
    } else {
        None
    }
}

/// Apple UAs use underscores in version numbers ("10_15_7"); render them
/// with dots so the result looks like the canonical version string.
fn normalize_apple_version(v: &str) -> String {
    v.replace('_', ".")
}

/// POST /settings/sessions/{id}/revoke - delete a specific session belonging
/// to the signed-in user. Refuses to revoke the current request's own
/// session (use logout for that) so a stray click here does not
/// invalidate the cookie the user is holding.
pub async fn post_session_revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(session_id): Path<String>,
    current: Option<axum::extract::Extension<CurrentSessionId>>,
) -> Result<Response, AppError> {
    if let Some(axum::extract::Extension(CurrentSessionId(cur))) = current {
        if cur == session_id {
            return Err(AppError::BadRequest(
                "Use Log out to end the current session.".to_string(),
            ));
        }
    }
    let removed = db::auth::delete_session_for_user(&state.auth, &session_id, &user.id).await?;
    if removed {
        state.last_seen_ledger.remove(&session_id);
    }
    Ok(Redirect::to("/settings?session_revoked=1").into_response())
}

/// Map a short error code carried in the redirect query string to a human
/// message. Codes (not full sentences) keep URLs tidy and let translations
/// live alongside other UI copy in the future.
fn password_error_message(code: &str) -> Option<&'static str> {
    match code {
        "incorrect" => Some("Current password is incorrect"),
        "short" => Some("New password must be at least 8 characters"),
        "mismatch" => Some("New passwords do not match"),
        "same" => Some("New password must differ from the current password"),
        _ => None,
    }
}

pub async fn post_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<SettingsForm>,
) -> Result<Response, AppError> {
    let enabled = form.read_receipts_enabled.is_some();
    db::auth::set_read_receipts_enabled(&state.auth, &user.id, enabled).await?;
    let is_public = form.is_profile_public.is_some();
    db::auth::set_profile_public(&state.auth, &user.id, is_public).await?;
    let browser = form.notify_browser_enabled.is_some();
    let sound = form.notify_sound_enabled.is_some();
    // Defend against a manually-crafted POST while push is unavailable: even
    // if the client checks the box, the column stays 0 unless the server has
    // VAPID keys ready.
    let push = form.notify_push_enabled.is_some() && state.push_available();
    db::auth::set_notification_prefs(&state.auth, &user.id, browser, sound, push).await?;
    Ok(Redirect::to("/settings").into_response())
}

pub async fn post_profile(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut display_name: Option<String> = None;
    let mut bio: Option<String> = None;
    let mut avatar_bytes: Option<Vec<u8>> = None;
    let mut email_present = false;
    let mut email: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        match field.name().unwrap_or("") {
            "display_name" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("display_name: {e}")))?;
                let trimmed = v.trim();
                if trimmed.chars().count() > MAX_DISPLAY_NAME_CHARS {
                    return Err(AppError::BadRequest(format!(
                        "display name exceeds {MAX_DISPLAY_NAME_CHARS} characters"
                    )));
                }
                display_name = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            "bio" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("bio: {e}")))?;
                let trimmed = v.trim();
                if trimmed.chars().count() > MAX_BIO_CHARS {
                    return Err(AppError::BadRequest(format!(
                        "bio exceeds {MAX_BIO_CHARS} characters"
                    )));
                }
                bio = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            "email" => {
                email_present = true;
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("email: {e}")))?;
                let trimmed = v.trim();
                if trimmed.is_empty() {
                    email = None;
                } else {
                    if trimmed.chars().count() > MAX_EMAIL_CHARS {
                        return Err(AppError::BadRequest(format!(
                            "email exceeds {MAX_EMAIL_CHARS} characters"
                        )));
                    }
                    if !looks_like_email(trimmed) {
                        return Err(AppError::BadRequest(
                            "email address is not valid".to_string(),
                        ));
                    }
                    email = Some(trimmed.to_string());
                }
            }
            "avatar" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("avatar: {e}")))?;
                if bytes.is_empty() {
                    continue;
                }
                if bytes.len() > MAX_AVATAR_BYTES {
                    return Err(AppError::BadRequest("avatar exceeds 1 MiB".to_string()));
                }
                avatar_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    db::auth::update_user_profile(
        &state.auth,
        &user.id,
        display_name.as_deref(),
        bio.as_deref(),
    )
    .await?;

    if email_present {
        // Snapshot the existing value so we can tell whether the address
        // genuinely changed - only then do we issue a new verification mail.
        let prev_email = db::auth::get_user_email(&state.auth, &user.id).await?;
        if let Err(e) = db::auth::set_user_email(&state.auth, &user.id, email.as_deref()).await {
            if matches!(&e, sqlx::Error::Database(d) if d.is_unique_violation()) {
                return Err(AppError::Conflict(
                    "That email address is already in use".to_string(),
                ));
            }
            return Err(e.into());
        }

        let changed = prev_email.as_deref() != email.as_deref();
        if changed {
            // Burn outstanding verification tokens tied to the prior
            // address; `set_user_email` already cleared `email_verified_at`
            // when the value actually changed.
            #[cfg(feature = "standalone")]
            db::email_verification::invalidate_all_for_user(&state.auth, &user.id).await?;
            #[cfg(feature = "standalone")]
            if let Some(addr) = email.as_deref() {
                if state.mail_available() {
                    crate::routes::email_verification::spawn_dispatch(
                        &state,
                        user.id.clone(),
                        addr.to_string(),
                    );
                }
            }
        }
    }

    if let Some(bytes) = avatar_bytes {
        let new_ext = sniff_image_ext(&bytes)
            .ok_or_else(|| AppError::BadRequest("avatar must be PNG, JPEG, or WebP".to_string()))?;
        let dir = db::avatars_dir();
        let final_path = dir.join(format!("{}.{}", user.id, new_ext));
        let tmp_path = dir.join(format!("{}.{}.tmp", user.id, new_ext));
        tokio::fs::write(&tmp_path, &bytes)
            .await
            .map_err(|e| AppError::Internal(format!("write avatar: {e}")))?;
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|e| AppError::Internal(format!("rename avatar: {e}")))?;
        if let Some(prev) = user.avatar_ext.as_deref() {
            if prev != new_ext {
                let prev_path = dir.join(format!("{}.{}", user.id, prev));
                let _ = tokio::fs::remove_file(prev_path).await;
            }
        }
        db::auth::set_user_avatar_ext(&state.auth, &user.id, Some(new_ext)).await?;
    }

    Ok(Redirect::to("/settings").into_response())
}

pub async fn get_blocked_list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    render_blocked_list(&state, &user, None, "").await
}

#[derive(Deserialize)]
pub struct BlockByUsernameForm {
    pub username: String,
}

/// POST /settings/blocked - block a user looked up by username. Lets the
/// caller block users with private profiles, who would not appear in the
/// public people-search results. Re-renders the page with an inline error
/// when the username is blank, missing, or refers to the caller themselves.
pub async fn post_block_by_username(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<BlockByUsernameForm>,
) -> Result<Response, AppError> {
    let raw = form.username.trim();
    let trimmed = raw.strip_prefix('@').unwrap_or(raw);
    if trimmed.is_empty() {
        return render_blocked_list(&state, &user, Some("Enter a username."), "")
            .await
            .map(|h| h.into_response());
    }
    let target = db::auth::find_user_by_username(&state.auth, trimmed).await?;
    let unknown_msg = format!("No user found with username @{trimmed}.");
    let Some(target) = target else {
        return render_blocked_list(&state, &user, Some(&unknown_msg), trimmed)
            .await
            .map(|h| h.into_response());
    };
    if target.id == user.id {
        return render_blocked_list(&state, &user, Some("You can't block yourself."), "")
            .await
            .map(|h| h.into_response());
    }
    // Privacy gate: only allow blocking by username when the target shares an
    // enclave with the caller. Public profiles bypass this check because
    // they're already discoverable through people-search. Without this gate,
    // the username form leaks the existence of every account regardless of
    // its privacy setting and lets an abuser pre-emptively block a private
    // user purely from a guessed username. The error message intentionally
    // matches the "user not found" branch so the response cannot
    // distinguish the two cases.
    if !target.is_profile_public
        && !db::enclave::users_share_enclave(&state.chat, &user.id, &target.id).await?
    {
        return render_blocked_list(&state, &user, Some(&unknown_msg), trimmed)
            .await
            .map(|h| h.into_response());
    }
    db::auth::block_user(&state.auth, &user.id, &target.id).await?;
    Ok(Redirect::to("/settings/blocked").into_response())
}

async fn render_blocked_list(
    state: &AppState,
    user: &crate::models::User,
    error: Option<&str>,
    form_username: &str,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(state, user, None).await?;
    let records = db::auth::list_blocked_users(&state.auth, &user.id).await?;
    let blocked: Vec<BlockedUserView> = records
        .into_iter()
        .map(|r| BlockedUserView {
            id: r.id,
            username: r.username,
            display_name: r.display_name,
            avatar_ext: r.avatar_ext,
        })
        .collect();
    let page = BlockedListPage {
        user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        blocked: &blocked,
        error,
        form_username,
    };
    html(&page)
}

#[cfg(feature = "standalone")]
#[derive(Deserialize)]
pub struct PasswordForm {
    pub current_password: String,
    pub new_password: String,
    pub new_password_confirm: String,
}

/// POST /settings/password - change the signed-in user's password. On any
/// validation failure redirect back to /settings with a `password_error`
/// query param; on success redirect with `password_changed=1` so the page
/// can show a confirmation banner. The current session cookie stays valid;
/// invalidating sibling sessions is a future improvement and is already
/// handled by the email-reset path.
#[cfg(feature = "standalone")]
pub async fn post_password(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<PasswordForm>,
) -> Result<Response, AppError> {
    let record = db::auth::find_user_by_id(&state.auth, &user.id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if !verify_password(&record.password_hash, &form.current_password) {
        return Ok(Redirect::to("/settings?password_error=incorrect").into_response());
    }
    if form.new_password.len() < MIN_PASSWORD_CHARS {
        return Ok(Redirect::to("/settings?password_error=short").into_response());
    }
    if form.new_password != form.new_password_confirm {
        return Ok(Redirect::to("/settings?password_error=mismatch").into_response());
    }
    if form.new_password == form.current_password {
        return Ok(Redirect::to("/settings?password_error=same").into_response());
    }
    let hash =
        hash_password(&form.new_password).map_err(|e| AppError::Internal(format!("hash: {e}")))?;
    db::auth::set_password_hash(&state.auth, &user.id, &hash).await?;
    Ok(Redirect::to("/settings?password_changed=1").into_response())
}

#[cfg(feature = "standalone")]
fn verify_password(hash: &str, password: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    use argon2::Argon2;
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(feature = "standalone")]
fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

pub async fn post_avatar_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    if let Some(ext) = user.avatar_ext.as_deref() {
        let path = db::avatars_dir().join(format!("{}.{}", user.id, ext));
        let _ = tokio::fs::remove_file(path).await;
    }
    db::auth::set_user_avatar_ext(&state.auth, &user.id, None).await?;
    Ok(Redirect::to("/settings").into_response())
}

/// Very loose syntactic check: requires exactly one `@`, non-empty local and
/// domain parts, a dot in the domain, and no whitespace. Real validity is
/// proven only by successful delivery; the goal here is to reject obvious
/// typos before they hit the SMTP transport.
fn looks_like_email(s: &str) -> bool {
    if s.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let mut parts = s.split('@');
    let local = match parts.next() {
        Some(l) if !l.is_empty() => l,
        _ => return false,
    };
    let domain = match parts.next() {
        Some(d) if !d.is_empty() => d,
        _ => return false,
    };
    if parts.next().is_some() {
        return false;
    }
    let _ = local;
    domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some("png");
    }
    if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        return Some("jpg");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

#[cfg(test)]
mod ua_tests {
    use super::summarize_user_agent;

    fn label(ua: &str) -> String {
        summarize_user_agent(Some(ua))
    }

    #[test]
    fn windows_chrome() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36";
        assert_eq!(label(ua), "Chrome on Windows 10/11");
    }

    #[test]
    fn windows_seven_firefox() {
        let ua = "Mozilla/5.0 (Windows NT 6.1; rv:109.0) Gecko/20100101 Firefox/115.0";
        assert_eq!(label(ua), "Firefox on Windows 7");
    }

    #[test]
    fn macos_safari_with_version() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.3 Safari/605.1.15";
        assert_eq!(label(ua), "Safari on macOS 14.3");
    }

    #[test]
    fn iphone_safari_with_version() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_3 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.3 Mobile/15E148 Safari/604.1";
        assert_eq!(label(ua), "Safari on iOS 17.3");
    }

    #[test]
    fn ipad_safari_with_version() {
        let ua = "Mozilla/5.0 (iPad; CPU OS 17_3 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.3 Mobile/15E148 Safari/604.1";
        assert_eq!(label(ua), "Safari on iPadOS 17.3");
    }

    #[test]
    fn android_chrome_with_version() {
        let ua = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Mobile Safari/537.36";
        assert_eq!(label(ua), "Chrome on Android 14");
    }

    #[test]
    fn chromeos_chrome() {
        // ChromeOS sits behind a generic X11 token with no "Linux" string;
        // the older code reported "Browser on Unknown OS" here.
        let ua = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36";
        assert_eq!(label(ua), "Chrome on ChromeOS");
    }

    #[test]
    fn linux_firefox() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0";
        assert_eq!(label(ua), "Firefox on Linux");
    }

    #[test]
    fn freebsd_firefox() {
        let ua = "Mozilla/5.0 (X11; FreeBSD amd64; rv:121.0) Gecko/20100101 Firefox/121.0";
        assert_eq!(label(ua), "Firefox on FreeBSD");
    }

    #[test]
    fn edge_on_windows() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36 Edg/121.0.0.0";
        assert_eq!(label(ua), "Edge on Windows 10/11");
    }

    #[test]
    fn x11_only_falls_back_to_unix() {
        let ua = "Mozilla/5.0 (X11; U; OpenIndiana) AppleWebKit/537.36";
        assert_eq!(label(ua), "Browser on Unix");
    }

    #[test]
    fn empty_or_unknown_stays_generic() {
        assert_eq!(label("curl/8.5"), "Browser on Unknown OS");
        assert_eq!(summarize_user_agent(None), "Unknown device");
    }
}
