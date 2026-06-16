#[allow(unused_imports)]
use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Template)]
#[template(path = "status/picker.html")]
struct PickerFragment<'a> {
    current_status: &'a str,
    current_custom: Option<&'a str>,
    /// LC-319: absolute ISO expiry of the active custom status, if scheduled.
    /// Drives the "auto-clears" hint; the `clear_after` select itself always
    /// defaults to "never" (the form is authoritative, Slack-style).
    current_expires_at: Option<String>,
}

/// LC-319: map a `clear_after` preset to a SQLite relative-time modifier for
/// `datetime('now', ?)`. The output is a fixed allowlist string, never raw
/// user input, so it is safe to interpolate into the time function. Anything
/// unrecognized (including the empty "never" choice) yields `None` = no expiry.
fn expiry_modifier(clear_after: &str) -> Option<&'static str> {
    match clear_after {
        "30m" => Some("+30 minutes"),
        "1h" => Some("+1 hours"),
        "4h" => Some("+4 hours"),
        "1d" => Some("+1 days"),
        "1w" => Some("+7 days"),
        _ => None,
    }
}

#[derive(Template)]
#[template(path = "status/own_avatar_oob.html")]
struct OwnAvatarOobFragment {
    user_id: String,
    username: String,
    avatar_ext: Option<String>,
    status: String,
    custom_status: Option<String>,
}

/// GET /status/picker - render the inline status picker for the caller.
pub async fn get_picker(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    // Only surface an expiry hint when there is actually a custom status; a
    // bare expiry on empty text is meaningless and the sweep would clear it.
    let current_expires_at = if user.custom_status.is_some() {
        db::auth::get_custom_status_expiry(&state.auth, &user.id)
            .await?
            .map(|ts| crate::views::room::to_iso_utc(&ts))
    } else {
        None
    };
    let frag = PickerFragment {
        current_status: &user.status,
        current_custom: user.custom_status.as_deref(),
        current_expires_at,
    };
    html(&frag)
}

/// GET /status/cancel - close the picker by emptying its target slot.
pub async fn cancel_picker() -> Response {
    axum::response::Html("").into_response()
}

#[derive(Deserialize)]
pub struct StatusForm {
    status: String,
    #[serde(default)]
    custom_status: String,
    /// LC-319: auto-clear preset (`30m`/`1h`/`4h`/`1d`/`1w`); empty / absent =
    /// never expire. Ignored when there is no custom text.
    #[serde(default)]
    clear_after: String,
}

/// POST /status - persist the caller's status + custom text, broadcast the
/// change so other viewers' sidebars refresh. Returns an empty body for the
/// `#status-picker` slot (closing the dialog) plus an OOB swap for the
/// caller's own avatar.
pub async fn post_status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<StatusForm>,
) -> Result<Html, AppError> {
    let custom_trimmed = form.custom_status.trim();
    let custom_opt: Option<&str> = if custom_trimmed.is_empty() {
        None
    } else {
        Some(custom_trimmed)
    };

    // An expiry without custom text is meaningless: only schedule a clear when
    // there is something to clear.
    let expires_modifier = if custom_opt.is_some() {
        expiry_modifier(&form.clear_after)
    } else {
        None
    };

    db::auth::set_user_status(
        &state.auth,
        &user.id,
        &form.status,
        custom_opt,
        expires_modifier,
    )
    .await
    .map_err(|e| match e {
        db::auth::SetStatusError::InvalidStatus => AppError::BadRequest("invalid status".into()),
        db::auth::SetStatusError::CustomTooLong(n) => {
            AppError::BadRequest(format!("custom status exceeds {n} characters"))
        }
        db::auth::SetStatusError::Db(e) => AppError::from(e),
    })?;

    let custom_owned = custom_opt.map(|s| s.to_string());
    state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
        user_id: user.id.clone(),
        status: form.status.clone(),
        custom_status: custom_owned.clone(),
    });

    let own_avatar = OwnAvatarOobFragment {
        user_id: user.id.clone(),
        username: user.username.clone(),
        avatar_ext: user.avatar_ext.clone(),
        status: form.status.clone(),
        custom_status: custom_owned,
    };
    Ok(Html(own_avatar.render()?))
}
