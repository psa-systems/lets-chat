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
    /// LC-319: absolute ISO expiry of the current status, if scheduled. LC-1057:
    /// when present the `clear_after` select preselects "Keep current", so
    /// saving again (e.g. only editing the text) does not wipe the timer.
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

/// GET /status/picker - render the inline status picker for the caller.
pub async fn get_picker(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    // LC-1057: the expiry covers the whole status, so it is shown whenever set.
    let current_expires_at = db::auth::get_custom_status_expiry(&state.auth, &user.id)
        .await?
        .map(|ts| crate::views::room::to_iso_utc(&ts));
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
    /// never expire; LC-1057: `keep` leaves a running timer as it is. Ignored
    /// for plain Online with no custom text, which has nothing to clear.
    #[serde(default)]
    clear_after: String,
}

/// POST /status - persist the caller's status + custom text and broadcast the
/// change so other viewers' avatars refresh. Returns an empty body for the
/// `#status-picker` slot (closing the dialog) plus an OOB refresh of the
/// caller's own `#sidebar-self` block (LC-1057: the full account-menu partial,
/// the same fragment a profile edit pushes, so the menu keeps working).
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

    // LC-1057: "Clear after" resets the whole status (Away / DND back to
    // Online, custom text cleared), so it applies whenever there is something
    // to reset: a non-Online status or custom text.
    let resettable = form.status != db::auth::STATUS_ACTIVE || custom_opt.is_some();
    let expires_modifier = if !resettable {
        None
    } else if form.clear_after == db::auth::KEEP_STATUS_EXPIRY {
        Some(db::auth::KEEP_STATUS_EXPIRY)
    } else {
        expiry_modifier(&form.clear_after)
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

    state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
        user_id: user.id.clone(),
        status: form.status.clone(),
        custom_status: custom_opt.map(|s| s.to_string()),
    });
    // The caller's other tabs refresh their own block the same way.
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::UserProfileChanged {
            user_id: user.id.clone(),
        },
    );

    let fresh: crate::models::User = db::auth::find_user_by_id(&state.auth, &user.id)
        .await?
        .ok_or(AppError::NotFound)?
        .into();
    Ok(Html(
        crate::views::ws_fragments::OwnProfileLiveFragment { user: &fresh }.render()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::expiry_modifier;

    #[test]
    fn expiry_presets_map_to_fixed_modifiers() {
        assert_eq!(expiry_modifier("30m"), Some("+30 minutes"));
        assert_eq!(expiry_modifier("1w"), Some("+7 days"));
        assert_eq!(expiry_modifier(""), None);
        assert_eq!(
            expiry_modifier("keep"),
            None,
            "keep is handled by the route"
        );
        assert_eq!(expiry_modifier("+1 years"), None, "never raw input");
    }
}
