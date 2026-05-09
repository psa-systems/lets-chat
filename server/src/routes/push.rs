use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// GET /sw.js - serve the service worker from root scope. Must NOT live
/// under /assets/ because the SW's registration scope is bounded by its
/// own URL.
pub async fn get_service_worker() -> Response {
    let body = include_str!("../../assets/sw.js");
    ([(header::CONTENT_TYPE, "application/javascript")], body).into_response()
}

/// GET /push/vapid-public-key - return the base64url-encoded raw P-256
/// public key. The page-side JS uses this as `applicationServerKey`
/// when calling `pushManager.subscribe`. 404 when push is disabled.
pub async fn get_vapid_public_key(State(state): State<AppState>) -> Result<Response, AppError> {
    let Some(kp) = state.vapid.as_ref() else {
        return Err(AppError::NotFound);
    };
    Ok(Json(serde_json::json!({ "key": kp.public_key_b64url })).into_response())
}

#[derive(Deserialize)]
pub struct SubscribeBody {
    pub endpoint: String,
    pub keys: SubscribeKeys,
}

#[derive(Deserialize)]
pub struct SubscribeKeys {
    pub p256dh: String,
    pub auth: String,
}

/// POST /push/subscribe - register or replace a Push subscription for
/// the authenticated user. Returns 204. 404 when push is disabled.
pub async fn post_subscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(body): Json<SubscribeBody>,
) -> Result<Response, AppError> {
    if state.vapid.is_none() {
        return Err(AppError::NotFound);
    }
    if body.endpoint.is_empty() || body.keys.p256dh.is_empty() || body.keys.auth.is_empty() {
        return Err(AppError::BadRequest("missing subscription fields".into()));
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    db::push_subscriptions::insert_or_replace(
        &state.auth,
        &user.id,
        &body.endpoint,
        &body.keys.p256dh,
        &body.keys.auth,
        user_agent.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
