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
///
/// The `__ASSET_VERSION__` token in sw.js is substituted with the build's
/// asset version (LC-98) so the SW's shell-cache name changes on every
/// deploy; its activate handler then evicts the stale cache. `no-cache`
/// keeps the browser revalidating the worker script itself on each load.
pub async fn get_service_worker(State(state): State<AppState>) -> Response {
    let body =
        include_str!("../../assets/sw.js").replace("__ASSET_VERSION__", &state.asset_version);
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
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

#[derive(Deserialize)]
pub struct ApnsRegisterBody {
    pub device_token: String,
    #[serde(default)]
    pub topic: Option<String>,
}

/// POST /push/apns - register or replace an APNs (iOS) device token for the
/// authenticated user. LC-91: tokens are accepted even before the server-side
/// APNs sender is configured, so a native client can register early and
/// delivery begins the moment credentials are wired. Returns 204.
pub async fn post_apns_register(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(body): Json<ApnsRegisterBody>,
) -> Result<Response, AppError> {
    if body.device_token.trim().is_empty() {
        return Err(AppError::BadRequest("missing device_token".into()));
    }
    let topic = body
        .topic
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    db::apns_subscriptions::insert_or_replace(
        &state.auth,
        &user.id,
        body.device_token.trim(),
        topic,
        user_agent.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub struct FcmRegisterBody {
    pub registration_token: String,
}

/// POST /push/fcm - register or replace an FCM (Android) registration token
/// for the authenticated user. Accepted before the FCM sender is configured;
/// see `post_apns_register`. Returns 204.
pub async fn post_fcm_register(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(body): Json<FcmRegisterBody>,
) -> Result<Response, AppError> {
    if body.registration_token.trim().is_empty() {
        return Err(AppError::BadRequest("missing registration_token".into()));
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    db::fcm_subscriptions::insert_or_replace(
        &state.auth,
        &user.id,
        body.registration_token.trim(),
        user_agent.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
