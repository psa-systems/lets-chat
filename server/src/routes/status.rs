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
    saved: bool,
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
pub async fn get_picker(AuthUser(user): AuthUser) -> Result<Html, AppError> {
    let frag = PickerFragment {
        current_status: &user.status,
        current_custom: user.custom_status.as_deref(),
        saved: false,
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
}

/// POST /status - persist the caller's status + custom text, broadcast the
/// change so other viewers' sidebars refresh, and re-render the picker with a
/// "Saved." confirmation.
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

    db::auth::set_user_status(&state.auth, &user.id, &form.status, custom_opt)
        .await
        .map_err(|e| match e {
            db::auth::SetStatusError::InvalidStatus => {
                AppError::BadRequest("invalid status".into())
            }
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

    let picker = PickerFragment {
        current_status: &form.status,
        current_custom: custom_owned.as_deref(),
        saved: true,
    };
    let picker_html = picker.render()?;
    let own_avatar = OwnAvatarOobFragment {
        user_id: user.id.clone(),
        username: user.username.clone(),
        avatar_ext: user.avatar_ext.clone(),
        status: form.status.clone(),
        custom_status: custom_owned,
    };
    let body = format!("{}{}", picker_html, own_avatar.render()?);
    Ok(Html(body))
}
