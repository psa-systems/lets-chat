use axum::extract::{Multipart, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::auth::{AuthUser, CurrentSessionId, SESSION_COOKIE};
use crate::db;
use crate::dnd;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::settings::{
    BlockedListPage, BlockedUserView, LocaleOption, SessionView, UserSettingsPage,
};
use crate::views::welcome::WelcomeHandlePage;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

const MAX_AVATAR_BYTES: usize = 1024 * 1024;
const MAX_DISPLAY_NAME_CHARS: usize = 64;
const MAX_BIO_CHARS: usize = 500;
const MAX_EMAIL_CHARS: usize = 254;
#[derive(Deserialize, Default)]
pub struct SettingsQuery {
    #[serde(default)]
    pub session_revoked: Option<String>,
    /// LC-100: set by the language-save redirect to flash "Saved.".
    #[serde(default)]
    pub saved: Option<String>,
    /// LC-356: inline error flash for the profile / delete-account forms, set
    /// by `settings_error_redirect` so a failed save keeps the user on the
    /// settings page instead of throwing a full-page error.
    #[serde(default)]
    pub error: Option<String>,
}

/// LC-356: redirect back to /settings with an inline error flash. `msg` is
/// always a fixed server-side validation string (never raw user input);
/// Askama escapes it on render. Replaces the previous `AppError::BadRequest`
/// full-page error on the profile and delete-account forms.
pub(crate) fn settings_error_redirect(msg: &str) -> Response {
    let encoded: String = url::form_urlencoded::byte_serialize(msg.as_bytes()).collect();
    Redirect::to(&format!("/settings?error={encoded}")).into_response()
}

/// LC-426: true when the request was issued by htmx (so a form handler should
/// return a small status fragment instead of a full-page redirect).
fn is_hx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

/// LC-426: HX-aware success. Returns the rendered feedback fragment for an htmx
/// request, or `None` so the caller falls back to its full-page redirect
/// (progressive enhancement for a no-JS submit).
fn hx_feedback(
    headers: &HeaderMap,
    fb: crate::views::settings::SettingsFeedback,
) -> Option<Result<Response, AppError>> {
    if !is_hx(headers) {
        return None;
    }
    Some(html(&fb).map(IntoResponse::into_response))
}

/// LC-426: HX-aware error. An error status fragment for htmx, else the existing
/// `?error=` redirect. `msg` is always a fixed server-side string.
fn settings_error(headers: &HeaderMap, msg: &str) -> Response {
    if is_hx(headers) {
        match html(&crate::views::settings::SettingsFeedback::err(
            msg.to_string(),
        )) {
            Ok(h) => h.into_response(),
            Err(_) => settings_error_redirect(msg),
        }
    } else {
        settings_error_redirect(msg)
    }
}

/// LC-347: cache-buster for the avatar preview `<img>`. The avatar route serves
/// `Cache-Control: max-age=300`, and the page-wide `asset_version` is a build
/// constant that never moves on upload, so without this the browser shows the
/// stale avatar for up to 5 minutes after a successful upload. Keying on the
/// avatar file's mtime makes the URL change exactly when the image does. Falls
/// back to `asset_version` when the user has no custom avatar (the `<img>` is
/// not rendered in that case, so the value only has to be stable).
async fn avatar_cache_key(user: &crate::models::User, asset_version: &str) -> String {
    let Some(ext) = user.avatar_ext.as_deref() else {
        return asset_version.to_string();
    };
    let path = db::avatars_dir().join(format!("{}.{}", user.id, ext));
    match tokio::fs::metadata(&path).await.and_then(|m| m.modified()) {
        Ok(mtime) => match mtime.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos().to_string(),
            Err(_) => asset_version.to_string(),
        },
        Err(_) => asset_version.to_string(),
    }
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
    #[serde(default)]
    pub notify_email_digest_enabled: Option<String>,
    #[serde(default)]
    pub notify_login_alerts_enabled: Option<String>,
    /// LC-77-REPLY: per-message mention + DM email opt-in. Separate from
    /// the digest opt-in because the cadence + content differ
    /// substantially (single-event vs hourly batch).
    #[serde(default)]
    pub notify_email_activity_enabled: Option<String>,
    /// LC-526 follow-up: opt out of the public kudos leaderboard.
    #[serde(default)]
    pub kudos_leaderboard_opt_out: Option<String>,
}

/// LC-88: the DND fields split out of a `UserRecord` for form rendering.
#[derive(Default)]
struct DndView {
    paused_until: String,
    timezone: String,
    weekday_start: String,
    weekday_end: String,
    weekend_start: String,
    weekend_end: String,
}

/// Split a stored DND schedule + pause into flat strings for the settings
/// form. Missing/invalid values render as empty (the "Off" state).
fn dnd_view_from(schedule_json: Option<&str>, paused_until: Option<&str>) -> DndView {
    let mut v = DndView {
        paused_until: paused_until.unwrap_or_default().to_string(),
        ..Default::default()
    };
    if let Some(s) = crate::dnd::Schedule::parse(schedule_json) {
        v.timezone = s.timezone;
        if let Some(w) = s.weekday {
            v.weekday_start = w.start;
            v.weekday_end = w.end;
        }
        if let Some(w) = s.weekend {
            v.weekend_start = w.start;
            v.weekend_end = w.end;
        }
    }
    v
}

/// IANA timezone options for the DND picker, with the user's current
/// selection pre-marked.
fn timezone_options(selected: &str) -> Vec<crate::views::settings::TzOption> {
    chrono_tz::TZ_VARIANTS
        .iter()
        .map(|tz| crate::views::settings::TzOption {
            name: tz.name(),
            selected: tz.name() == selected,
        })
        .collect()
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<SettingsQuery>,
    jar: CookieJar,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let email = db::auth::get_user_email(&state.auth, &user.id).await?;
    // LC-514: `current_session` is the raw cookie value; the row id is
    // now the SHA-256 hash. Compare hashes when marking `is_current` so
    // the settings UI keeps highlighting the row that issued the request.
    let current_session = jar
        .get(SESSION_COOKIE)
        .map(|c| db::auth::hash_session_token(c.value()));
    let sessions = build_session_views(&state, &user.id, current_session.as_deref()).await?;
    let storage_usage_bytes = db::quota::sum_user_usage(&state.chat, &user.id).await?;
    let storage_quota_bytes = db::quota::get_user_quota(&state.chat, &user.id).await?;
    let storage_usage_display =
        format!("{:.2} MiB", storage_usage_bytes as f64 / (1024.0 * 1024.0));
    let storage_quota_display =
        storage_quota_bytes.map(|b| format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0)));
    // LC-88: read the raw DND columns (the public `User` only carries the
    // computed `dnd_active` flag, not the stored schedule/pause) to render the
    // editor in its current state.
    let dnd = match db::auth::find_user_by_id(&state.auth, &user.id).await? {
        Some(rec) => dnd_view_from(
            rec.dnd_schedule_json.as_deref(),
            rec.dnd_paused_until.as_deref(),
        ),
        None => DndView::default(),
    };
    let timezones = timezone_options(&dnd.timezone);
    // LC-100: language picker options. The empty-code entry is "use browser
    // language" (clears the saved preference).
    let mut locales = vec![LocaleOption {
        code: String::new(),
        name: crate::i18n::translate_current("settings-language-system"),
        selected: user.locale.is_none(),
    }];
    for l in crate::i18n::available() {
        let code = l.to_string();
        let native = crate::i18n::native_name(&code);
        let name = if native.is_empty() {
            code.clone()
        } else {
            native.to_string()
        };
        let selected = user.locale.as_deref() == Some(code.as_str());
        locales.push(LocaleOption {
            code,
            name,
            selected,
        });
    }

    let keywords = db::notification_keywords::list(&state.auth, &user.id).await?;
    // LC-482: the user's personal custom emoji for the Custom-emoji panel.
    let personal_emojis = db::custom_emojis::list_for_user(&state.chat, &user.id).await?;
    // LC-487: the user's canned responses for the Saved-replies panel.
    let canned_responses = db::slash::list_canned_for_user(&state.chat, &user.id).await?;
    let avatar_version = avatar_cache_key(&user, &state.asset_version).await;

    // LC-766: is the handle free to change now, or still on its cooldown? Admins
    // are exempt. The date drives the "again on ..." help line when locked.
    let handle_cooldown_until =
        db::auth::handle_change_available_on(&state.auth, &user.id, user.role == "admin").await?;
    let handle_editable = handle_cooldown_until.is_none();
    let handle_available_on = handle_cooldown_until
        .as_deref()
        .and_then(|s| s.split(' ').next())
        .unwrap_or("")
        .to_string();

    let page = UserSettingsPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        avatar_version,
        saved: q.saved.is_some(),
        error: q.error.clone(),
        push_available: state.push_available(),
        email,
        email_available: state.mail_available(),
        kudos_opt_out: db::auth::get_kudos_opt_out(&state.auth, &user.id).await?,
        sessions: &sessions,
        session_revoked: q.session_revoked.is_some(),
        storage_usage_display,
        storage_quota_display,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        dnd_active: user.dnd_active,
        dnd_paused_until: dnd.paused_until,
        dnd_timezone: dnd.timezone,
        dnd_weekday_start: dnd.weekday_start,
        dnd_weekday_end: dnd.weekday_end,
        dnd_weekend_start: dnd.weekend_start,
        dnd_weekend_end: dnd.weekend_end,
        timezones,
        profile_timezone: user.timezone.clone().unwrap_or_default(),
        locales,
        keywords,
        personal_emojis,
        emoji_max_kib: crate::routes::custom_emojis::MAX_EMOJI_BYTES / 1024,
        canned_responses,
        handle_editable,
        handle_available_on,
    };
    html(&page)
}

#[derive(Deserialize)]
pub struct KeywordForm {
    #[serde(default)]
    pub word: String,
}

/// LC-304: re-render the highlight-words chip list fragment for the caller.
async fn keyword_list_fragment(state: &AppState, user_id: &str) -> Result<Html, AppError> {
    let keywords = db::notification_keywords::list(&state.auth, user_id).await?;
    let at_cap = keywords.len() >= db::notification_keywords::MAX_KEYWORDS_PER_USER;
    html(&crate::views::settings::KeywordListFragment { keywords, at_cap })
}

/// POST /settings/keywords - add a highlight word. Trims, lowercases nothing
/// (stored as entered; matched case-insensitively), bounds length, and enforces
/// the per-user cap. Returns the re-rendered chip list. A blank or over-cap add
/// is a no-op that still returns the current list.
pub async fn post_keyword_add(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<KeywordForm>,
) -> Result<Html, AppError> {
    let word = form.word.trim();
    let mut added = false;
    if !word.is_empty()
        && word.chars().count() <= db::notification_keywords::MAX_KEYWORD_LEN
        && db::notification_keywords::count(&state.auth, &user.id).await?
            < db::notification_keywords::MAX_KEYWORDS_PER_USER as i64
    {
        db::notification_keywords::add(&state.auth, &user.id, word).await?;
        added = true;
    }
    // LC-426: the chip appears immediately (the swap), but pop a confirmation
    // toast too so it matches the rest of the page. OOB block rides along with
    // the re-rendered list. Removal is left to the visible chip disappearing.
    let mut frag = keyword_list_fragment(&state, &user.id).await?;
    if added {
        let toast = html(&crate::views::settings::SettingsFeedback::toast_only_ok(
            crate::i18n::translate_current("settings-fb-keyword-added"),
        ))?;
        frag.0.push_str(&toast.0);
    }
    Ok(frag)
}

/// POST /settings/keywords/delete - remove a highlight word. Returns the
/// re-rendered chip list.
pub async fn post_keyword_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<KeywordForm>,
) -> Result<Html, AppError> {
    db::notification_keywords::remove(&state.auth, &user.id, form.word.trim()).await?;
    keyword_list_fragment(&state, &user.id).await
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
pub(crate) fn summarize_user_agent(ua: Option<&str>) -> String {
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
    headers: HeaderMap,
    current: Option<axum::extract::Extension<CurrentSessionId>>,
) -> Result<Response, AppError> {
    if let Some(axum::extract::Extension(CurrentSessionId(cur))) = current {
        // LC-514: `session_id` here is the row id, which is now the
        // SHA-256 hex of the original cookie. `CurrentSessionId` still
        // carries the raw cookie value, so hash it before comparing.
        if db::auth::hash_session_token(&cur) == session_id {
            return Err(AppError::BadRequest(
                "Use Log out to end the current session.".to_string(),
            ));
        }
    }
    let removed = db::auth::delete_session_for_user(&state.auth, &session_id, &user.id).await?;
    if removed {
        state.last_seen_ledger.remove(&session_id);
    }
    // LC-426: htmx targets the row (outerHTML); the toast-only fragment removes
    // it (empty main content) and pops a confirmation toast.
    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::toast_only_ok(crate::i18n::translate_current(
            "settings-fb-session-revoked",
        )),
    ) {
        return r;
    }
    Ok(Redirect::to("/settings?session_revoked=1").into_response())
}

/// Map a short error code carried in the redirect query string to a human
#[derive(serde::Deserialize)]
pub struct LanguageForm {
    /// Locale code, or empty for "use browser language" (clears the setting).
    #[serde(default)]
    pub locale: String,
}

/// POST /settings/language - save the UI locale preference. An empty or
/// unsupported value clears the preference (falls back to Accept-Language).
pub async fn post_language(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    axum::Form(form): axum::Form<LanguageForm>,
) -> Result<Response, AppError> {
    let code = form.locale.trim();
    let to_set = if !code.is_empty() && crate::i18n::is_supported(code) {
        Some(code)
    } else {
        None
    };
    db::auth::set_user_locale(&state.auth, &user.id, to_set).await?;
    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::ok(crate::i18n::translate_current(
            "settings-fb-saved",
        )),
    ) {
        return r;
    }
    Ok(Redirect::to("/settings?saved=1").into_response())
}

#[derive(serde::Deserialize)]
pub struct AppearanceForm {
    /// "system" / "light" / "dark" / "hc-light" / "hc-dark".
    #[serde(default)]
    pub theme: String,
    /// LC-194: "comfortable" / "compact".
    #[serde(default)]
    pub density: String,
    /// LC-569: "compact" / "default" / "large" / "xl".
    #[serde(default)]
    pub scale: String,
    /// LC-541: "blue-harbor" / "cobalt" / "ink-ice" / "arctic" / "deep-sea" / "royal-navy" / "amethyst".
    #[serde(default)]
    pub palette: String,
    /// LC-575: "last-room" / "home" - the "Open on" landing preference.
    #[serde(default)]
    pub home_landing: String,
}

/// POST /settings/appearance - save the UI mode + palette + density
/// preferences. The locale middleware syncs the lc-mode / lc-palette /
/// lc-density cookies from these, so the no-flash bootstrap picks them up on
/// the redirect.
pub async fn post_appearance(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    axum::Form(form): axum::Form<AppearanceForm>,
) -> Result<Response, AppError> {
    let theme = match form.theme.trim() {
        t @ ("light" | "dark" | "hc-light" | "hc-dark" | "system") => t,
        _ => "system",
    };
    let density = match form.density.trim() {
        d @ ("comfortable" | "compact") => d,
        _ => "comfortable",
    };
    let palette = match form.palette.trim() {
        p @ ("blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy"
        | "amethyst") => Some(p),
        _ => None, // empty or unknown -> clear -> blue-harbor default
    };
    db::auth::set_user_theme_mode(&state.auth, &user.id, Some(theme)).await?;
    db::auth::set_user_density(&state.auth, &user.id, Some(density)).await?;
    db::auth::set_user_theme_palette(&state.auth, &user.id, palette).await?;
    let scale = match form.scale.trim() {
        s @ ("compact" | "default" | "large" | "xl") => s,
        _ => "default",
    };
    db::auth::set_user_theme_scale(&state.auth, &user.id, Some(scale)).await?;
    let home_landing = match form.home_landing.trim() {
        l @ ("last-room" | "home") => l,
        _ => "last-room",
    };
    db::auth::set_user_home_landing(&state.auth, &user.id, Some(home_landing)).await?;
    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::ok(crate::i18n::translate_current(
            "settings-fb-saved",
        )),
    ) {
        return r;
    }
    Ok(Redirect::to("/settings?saved=1").into_response())
}

#[derive(serde::Deserialize)]
pub struct ThemeForm {
    /// "system" / "light" / "dark" / "hc-light" / "hc-dark".
    #[serde(default)]
    pub theme: String,
}

/// LC-292: POST /settings/theme - the sidebar quick-toggle's persist endpoint.
/// Saves only the theme (no density), and returns 204 with a `lc-mode` cookie
/// so the very next navigation reads the correct value (no flash-of-old-theme).
/// The no-flash bootstrap + `resolve_locale` middleware otherwise mirror this
/// cookie from `users.theme_mode`; setting it here just closes the one-load gap
/// for the fire-and-forget fetch path (the redirect-based appearance form gets
/// this for free). An invalid value falls back to "system" rather than 500ing.
pub async fn post_theme(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<ThemeForm>,
) -> Result<Response, AppError> {
    let theme = match form.theme.trim() {
        t @ ("light" | "dark" | "hc-light" | "hc-dark" | "system") => t,
        _ => "system",
    };
    db::auth::set_user_theme_mode(&state.auth, &user.id, Some(theme)).await?;
    let cookie = format!("lc-mode={theme}; Path=/; Max-Age=31536000; SameSite=Lax");
    Ok((
        axum::http::StatusCode::NO_CONTENT,
        [(axum::http::header::SET_COOKIE, cookie)],
    )
        .into_response())
}

#[derive(serde::Deserialize)]
pub struct PaletteForm {
    /// "blue-harbor" / "cobalt" / "ink-ice" / "arctic" / "deep-sea" / "royal-navy" / "amethyst".
    #[serde(default)]
    pub palette: String,
}

/// LC-541: POST /settings/palette - the appearance picker's palette persist.
/// Returns 204 + an `lc-palette` cookie so the next navigation has no flash.
pub async fn post_palette(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<PaletteForm>,
) -> Result<Response, AppError> {
    let palette = match form.palette.trim() {
        p @ ("blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy"
        | "amethyst") => p,
        _ => "blue-harbor",
    };
    db::auth::set_user_theme_palette(&state.auth, &user.id, Some(palette)).await?;
    let cookie = format!("lc-palette={palette}; Path=/; Max-Age=31536000; SameSite=Lax");
    Ok((
        axum::http::StatusCode::NO_CONTENT,
        [(axum::http::header::SET_COOKIE, cookie)],
    )
        .into_response())
}

#[derive(serde::Deserialize)]
pub struct ScaleForm {
    /// "compact" / "default" / "large" / "xl".
    #[serde(default)]
    pub scale: String,
}

/// LC-569: POST /settings/scale - the appearance picker's UI-scale persist.
/// Returns 204 + an `lc-scale` cookie so the next navigation has no flash.
pub async fn post_scale(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<ScaleForm>,
) -> Result<Response, AppError> {
    let scale = match form.scale.trim() {
        s @ ("compact" | "default" | "large" | "xl") => s,
        _ => "default",
    };
    db::auth::set_user_theme_scale(&state.auth, &user.id, Some(scale)).await?;
    let cookie = format!("lc-scale={scale}; Path=/; Max-Age=31536000; SameSite=Lax");
    Ok((
        axum::http::StatusCode::NO_CONTENT,
        [(axum::http::header::SET_COOKIE, cookie)],
    )
        .into_response())
}

pub async fn post_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
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
    // Same defence for digest: respect the operator gate even if the
    // client lies. Without a configured email transport at startup,
    // flipping the checkbox would silently do nothing useful anyway.
    let email_digest = form.notify_email_digest_enabled.is_some() && state.mail_available();
    db::auth::set_notification_prefs(&state.auth, &user.id, browser, sound, push, email_digest)
        .await?;
    // Same operator-gate as the digest: persist the user's chosen value
    // regardless of SMTP availability so flipping the toggle survives a
    // future SMTP enablement, but the dispatch path is already a no-op
    // when mail is unavailable.
    let login_alerts = form.notify_login_alerts_enabled.is_some();
    db::auth::set_notify_login_alerts_enabled(&state.auth, &user.id, login_alerts).await?;
    // LC-77-REPLY: same operator-gate posture as the digest. Persist the
    // user's chosen value even when mail is unavailable so the toggle
    // survives a future SMTP enablement; the dispatch path is already a
    // no-op when state.mailer is None.
    let email_activity = form.notify_email_activity_enabled.is_some() && state.mail_available();
    db::auth::set_notify_email_activity_enabled(&state.auth, &user.id, email_activity).await?;
    db::auth::set_kudos_opt_out(
        &state.auth,
        &user.id,
        form.kudos_leaderboard_opt_out.is_some(),
    )
    .await?;

    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::ok(crate::i18n::translate_current(
            "settings-fb-saved",
        )),
    ) {
        return r;
    }
    Ok(Redirect::to("/settings").into_response())
}

#[derive(Deserialize)]
pub struct DndScheduleForm {
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub weekday_start: String,
    #[serde(default)]
    pub weekday_end: String,
    #[serde(default)]
    pub weekend_start: String,
    #[serde(default)]
    pub weekend_end: String,
}

/// Validate a `"HH:MM"` pair into a `Window`, returning `None` when either
/// side is blank or malformed so a partial/garbage entry disables that group
/// instead of persisting an unusable window.
fn parse_window(start: &str, end: &str) -> Option<dnd::Window> {
    let (s, e) = (start.trim(), end.trim());
    if s.is_empty() || e.is_empty() {
        return None;
    }
    let valid = |x: &str| chrono::NaiveTime::parse_from_str(x, "%H:%M").is_ok();
    if !valid(s) || !valid(e) {
        return None;
    }
    Some(dnd::Window {
        start: s.to_string(),
        end: e.to_string(),
    })
}

/// LC-88: save (or clear) the recurring DND schedule. An empty/invalid
/// timezone, or a timezone with no usable windows, clears the schedule.
pub async fn post_dnd_schedule(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    axum::Form(form): axum::Form<DndScheduleForm>,
) -> Result<Response, AppError> {
    let tz = form.timezone.trim();
    let schedule_json = if tz.is_empty() || tz.parse::<chrono_tz::Tz>().is_err() {
        None
    } else {
        let weekday = parse_window(&form.weekday_start, &form.weekday_end);
        let weekend = parse_window(&form.weekend_start, &form.weekend_end);
        if weekday.is_none() && weekend.is_none() {
            // Timezone set but no windows: nothing to suppress, so store NULL
            // rather than an inert schedule object.
            None
        } else {
            let schedule = dnd::Schedule {
                timezone: tz.to_string(),
                weekday,
                weekend,
            };
            Some(
                serde_json::to_string(&schedule)
                    .map_err(|e| AppError::Internal(format!("dnd schedule serialize: {e}")))?,
            )
        }
    };
    db::auth::set_dnd_schedule(&state.auth, &user.id, schedule_json.as_deref()).await?;
    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::ok(crate::i18n::translate_current(
            "settings-fb-saved",
        )),
    ) {
        return r;
    }
    Ok(Redirect::to("/settings").into_response())
}

#[derive(Deserialize)]
pub struct DndPauseForm {
    pub minutes: i64,
}

/// LC-88: pause notifications for a fixed duration. Auto-expires by storing a
/// future instant; no sweeper needed. Capped at 7 days so a stray value can
/// never silence a user indefinitely.
pub async fn post_dnd_pause(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<DndPauseForm>,
) -> Result<Response, AppError> {
    let minutes = form.minutes.clamp(1, 7 * 24 * 60);
    let until = chrono::Utc::now() + chrono::Duration::minutes(minutes);
    db::auth::set_dnd_pause(&state.auth, &user.id, Some(&until.to_rfc3339())).await?;
    Ok(Redirect::to("/settings").into_response())
}

/// LC-88: clear an active manual pause (resume notifications now).
pub async fn post_dnd_resume(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    db::auth::set_dnd_pause(&state.auth, &user.id, None).await?;
    Ok(Redirect::to("/settings").into_response())
}

pub async fn post_profile(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut display_name: Option<String> = None;
    let mut bio: Option<String> = None;
    // LC-533: profile extras. Validated inline as the fields arrive.
    let mut pronouns: Option<String> = None;
    let mut profile_links: Option<String> = None;
    let mut timezone: Option<String> = None;
    let mut avatar_bytes: Option<Vec<u8>> = None;
    let mut email_present = false;
    let mut email: Option<String> = None;
    // LC-766: the chat handle, edited alongside display name in the same form.
    let mut username_field: Option<String> = None;
    let mut avatar_saved = false; // LC-426: drives the "picture updated" message

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
                    return Ok(settings_error(
                        &headers,
                        &format!("Display name exceeds {MAX_DISPLAY_NAME_CHARS} characters."),
                    ));
                }
                display_name = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            // LC-766: the chat handle. Captured here; validated + applied after
            // the loop so a handle refusal never half-writes the other fields.
            "username" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("username: {e}")))?;
                username_field = Some(v);
            }
            "bio" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("bio: {e}")))?;
                let trimmed = v.trim();
                if trimmed.chars().count() > MAX_BIO_CHARS {
                    return Ok(settings_error(
                        &headers,
                        &format!("Bio exceeds {MAX_BIO_CHARS} characters."),
                    ));
                }
                bio = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            // LC-533: short pronoun string.
            "pronouns" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("pronouns: {e}")))?;
                let trimmed = v.trim();
                if trimmed.chars().count() > crate::models::user::MAX_PRONOUNS_CHARS {
                    return Ok(settings_error(
                        &headers,
                        &format!(
                            "Pronouns exceed {} characters.",
                            crate::models::user::MAX_PRONOUNS_CHARS
                        ),
                    ));
                }
                pronouns = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            // LC-533: one http(s) URL per line; validated + normalised.
            "profile_links" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("profile_links: {e}")))?;
                match crate::models::user::validate_profile_links(&v) {
                    Ok(links) => profile_links = links,
                    Err(msg) => return Ok(settings_error(&headers, &msg)),
                }
            }
            // LC-533: IANA timezone for the profile "local time" line. Empty
            // clears it; a non-empty value must be a known zone.
            "timezone" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("timezone: {e}")))?;
                let trimmed = v.trim();
                if trimmed.is_empty() {
                    timezone = None;
                } else if trimmed.parse::<chrono_tz::Tz>().is_ok() {
                    timezone = Some(trimmed.to_string());
                } else {
                    return Ok(settings_error(&headers, "Unknown timezone."));
                }
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
                        return Ok(settings_error(
                            &headers,
                            &format!("Email exceeds {MAX_EMAIL_CHARS} characters."),
                        ));
                    }
                    if !looks_like_email(trimmed) {
                        return Ok(settings_error(&headers, "Email address is not valid."));
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
                    return Ok(settings_error(&headers, "Avatar image exceeds 1 MiB."));
                }
                avatar_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    // LC-766: apply a handle change first, so a refusal (taken / reserved /
    // cooldown / malformed) returns before any other field is written. An
    // unchanged handle is a no-op that never burns the cooldown.
    if let Some(raw) = username_field {
        let handle = match crate::models::user::validate_handle(&raw) {
            Ok(h) => h,
            Err(msg) => return Ok(settings_error(&headers, &msg)),
        };
        if handle != user.username {
            let admin = user.role == "admin";
            match db::auth::change_username(&state.auth, &user.id, &handle, true, !admin).await {
                Ok(change) => emit_handle_changed(&state, &user.id, &change),
                Err(db::auth::ChangeHandleError::Taken) => {
                    return Ok(settings_error(
                        &headers,
                        &format!("The handle @{handle} is already taken."),
                    ));
                }
                Err(db::auth::ChangeHandleError::Reserved) => {
                    return Ok(settings_error(
                        &headers,
                        &format!("The handle @{handle} is reserved right now. Choose another."),
                    ));
                }
                Err(db::auth::ChangeHandleError::Cooldown { available_on }) => {
                    let date = available_on.split(' ').next().unwrap_or(&available_on);
                    return Ok(settings_error(
                        &headers,
                        &format!("You can change your handle again on {date}."),
                    ));
                }
                Err(db::auth::ChangeHandleError::Db(e)) => return Err(e.into()),
            }
        }
    }

    db::auth::update_user_profile(
        &state.auth,
        &user.id,
        display_name.as_deref(),
        bio.as_deref(),
        pronouns.as_deref(),
        profile_links.as_deref(),
        timezone.as_deref(),
    )
    .await?;

    if email_present {
        if let Err(e) = db::auth::set_user_email(&state.auth, &user.id, email.as_deref()).await {
            if matches!(&e, sqlx::Error::Database(d) if d.is_unique_violation()) {
                return Ok(settings_error(
                    &headers,
                    "That email address is already in use.",
                ));
            }
            return Err(e.into());
        }
        // LC-22 cutover: email verification was retired with the password
        // path. `set_user_email` clears `email_verified_at` on change; the
        // verified flag survives as a column but is no longer maintained.
    }

    if let Some(bytes) = avatar_bytes {
        let Some(new_ext) = sniff_image_ext(&bytes) else {
            return Ok(settings_error(
                &headers,
                "Avatar must be a PNG, JPEG, or WebP image.",
            ));
        };
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
        // LC-781 (F11): the file's mtime just changed, so drop the cached version
        // token; the next render re-stats and emits a fresh `?v=`, busting the
        // browser's immutable cache.
        crate::avatar_version::invalidate(&user.id);
        avatar_saved = true;
    }

    // LC-173: refresh the editor's sidebar self block (avatar + name) in every
    // one of their other tabs. broadcast_to_user fans to all of this user's
    // connections; the WS send task gates on the recipient being the editor and
    // re-renders #sidebar-self OOB. Cross-surface refresh of how OTHER users
    // see this profile (message author rows, DM headers) is a follow-up.
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::UserProfileChanged {
            user_id: user.id.clone(),
        },
    );

    // LC-426: confirm inline + toast. A picture change gets its own message so
    // the upload (which has no other on-page confirmation) is unmistakable.
    let key = if avatar_saved {
        "settings-fb-avatar"
    } else {
        "settings-fb-profile"
    };
    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::ok(crate::i18n::translate_current(key)),
    ) {
        return r;
    }
    // LC-347: flash "Saved." on the full-page (no-JS) return.
    Ok(Redirect::to("/settings?saved=1").into_response())
}

/// LC-766: the identity-changed seam. Let's Chat owns the chat handle; Bunyip
/// has no handle concept, so there is nothing to mirror back. We emit a
/// structured event so a future Bunyip reconcile (or an audit) can observe every
/// change, and refresh the user's own tabs so their sidebar shows the new handle
/// at once.
fn emit_handle_changed(state: &AppState, user_id: &str, change: &db::auth::HandleChange) {
    tracing::info!(
        target: "identity_changed",
        user_id,
        old_handle = %change.old,
        new_handle = %change.new,
        "chat handle changed",
    );
    state.hub.broadcast_to_user(
        user_id,
        &ChatEvent::UserProfileChanged {
            user_id: user_id.to_string(),
        },
    );
}

/// Render the first-entry handle prompt, pre-filled with `handle` and carrying
/// an optional validation `error`.
async fn render_welcome_handle(
    state: &AppState,
    handle: &str,
    error: Option<&str>,
) -> Result<Html, AppError> {
    let branding = db::branding::resolve(&state.chat, db::branding::Scope::Global).await?;
    let page = WelcomeHandlePage {
        asset_version: &state.asset_version,
        handle,
        error,
        brand_logo: branding.logo_upload_id.is_some(),
        brand_heading: branding.login_heading,
    };
    html(&page)
}

/// GET /welcome/handle - the first-entry prompt (LC-766). A user who already
/// confirmed a handle is bounced home; the gate never sends them here.
pub async fn get_welcome_handle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    if user.username_confirmed_at.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(render_welcome_handle(&state, &user.username, None)
        .await?
        .into_response())
}

#[derive(Deserialize)]
pub struct WelcomeHandleForm {
    pub username: String,
}

/// POST /welcome/handle - validate + record the chosen handle, then release the
/// gate by sending the user home (LC-766). The first-entry pick is not a
/// "change": it starts no cooldown and reserves no handle (the derived value it
/// replaces was never anyone's).
pub async fn post_welcome_handle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<WelcomeHandleForm>,
) -> Result<Response, AppError> {
    if user.username_confirmed_at.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    let handle = match crate::models::user::validate_handle(&form.username) {
        Ok(h) => h,
        Err(msg) => {
            return Ok(render_welcome_handle(&state, &form.username, Some(&msg))
                .await?
                .into_response());
        }
    };
    // Accepting the derived handle unchanged: just record the confirmation.
    if handle == user.username {
        db::auth::confirm_username(&state.auth, &user.id).await?;
        return Ok(Redirect::to("/").into_response());
    }
    match db::auth::change_username(&state.auth, &user.id, &handle, false, false).await {
        Ok(change) => {
            emit_handle_changed(&state, &user.id, &change);
            Ok(Redirect::to("/").into_response())
        }
        Err(db::auth::ChangeHandleError::Taken) => {
            Ok(
                render_welcome_handle(&state, &handle, Some("That handle is already taken."))
                    .await?
                    .into_response(),
            )
        }
        Err(db::auth::ChangeHandleError::Reserved) => Ok(render_welcome_handle(
            &state,
            &handle,
            Some("That handle is reserved right now. Choose another."),
        )
        .await?
        .into_response()),
        // First entry passes enforce_cooldown = false, so this arm is unreachable
        // in practice; treat it defensively as a generic retry rather than panic.
        Err(db::auth::ChangeHandleError::Cooldown { .. }) => Ok(render_welcome_handle(
            &state,
            &handle,
            Some("That handle is not available. Choose another."),
        )
        .await?
        .into_response()),
        Err(db::auth::ChangeHandleError::Db(e)) => Err(e.into()),
    }
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
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
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
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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

// LC-22 cutover: password change handler + helpers deleted. Bunyip owns
// credential management; lets-chat no longer has a password to change.
pub async fn post_avatar_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(ext) = user.avatar_ext.as_deref() {
        let path = db::avatars_dir().join(format!("{}.{}", user.id, ext));
        let _ = tokio::fs::remove_file(path).await;
    }
    db::auth::set_user_avatar_ext(&state.auth, &user.id, None).await?;
    // LC-781 (F11): custom avatar gone; drop the cached token so renders fall
    // back to the default SVG's URL rather than a stale immutable custom one.
    crate::avatar_version::invalidate(&user.id);
    // LC-173: refresh the editor's other tabs (sidebar self block).
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::UserProfileChanged {
            user_id: user.id.clone(),
        },
    );
    // LC-432: confirm + revert the single on-page preview <img> to the generated
    // default. A fresh cache-buster is required because the file is now gone, so
    // the avatar route's mtime key no longer moves the URL off the cached custom
    // image.
    let avatar_src = format!(
        "/avatars/{}?v=del{}",
        user.id,
        chrono::Utc::now().timestamp_millis()
    );
    if let Some(r) = hx_feedback(
        &headers,
        crate::views::settings::SettingsFeedback::ok_reset_avatar(
            crate::i18n::translate_current("settings-fb-avatar-removed"),
            avatar_src,
        ),
    ) {
        return r;
    }
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
