//! LC-733: hand the desktop app a registry credential for the signed-in user.
//!
//! Let's Chat desktop binaries are membership-gated, so the self-updater has to
//! prove membership to pull them. The user already proved it at sign-in: the
//! Bunyip SSO callback (`bunyip_sso::get_callback`) receives an access token for
//! them, which this module remembers in memory and `GET /desktop/registry-token`
//! returns to that user's own authenticated session. The desktop bridge
//! (`desktop/src/inject.rs`) reads the route and forwards the token to the
//! native updater, so there is no second sign-in and no pasted credential.
//!
//! Standalone-only, like the Bunyip sign-in that feeds it: the saas build has
//! no Bunyip login, so it has nothing to hand out and does not serve the route.
//!
//! The store is in-memory on purpose: the token is short-lived, it is a
//! credential we would rather not write to disk, and losing it on restart costs
//! the user nothing worse than a page reload after their next sign-in. A session
//! whose token is gone gets a 503 that says exactly that, never a blank success.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::auth::OptionalUser;
use crate::state::AppState;

/// Fallback lifetime when the OP does not say how long its token lives. Short:
/// a stale credential is worse than a re-fetch, and the page re-fetches on
/// every load.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// Never hold a token past this, whatever the OP claims.
const MAX_TTL: Duration = Duration::from_secs(12 * 3600);

struct StoredToken {
    token: String,
    expires_at: Instant,
}

fn store() -> &'static Mutex<HashMap<String, StoredToken>> {
    static STORE: OnceLock<Mutex<HashMap<String, StoredToken>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember the Bunyip access token minted for `user_id` at sign-in. Called
/// from the SSO callback.
pub fn remember(user_id: &str, token: &str, expires_in: Option<i64>) {
    let ttl = expires_in
        .filter(|s| *s > 0)
        .map(|s| Duration::from_secs(s as u64))
        .unwrap_or(DEFAULT_TTL)
        .min(MAX_TTL);
    let Some(expires_at) = Instant::now().checked_add(ttl) else {
        tracing::warn!(target: "desktop_update", user_id, "token TTL overflowed; not stored");
        return;
    };
    let mut map = match store().lock() {
        Ok(m) => m,
        Err(poisoned) => {
            // A panic elsewhere poisoned the lock. The map itself is still
            // consistent (only inserts and removals happen under it), so
            // recover rather than propagate a panic into the login path.
            tracing::warn!(target: "desktop_update", "token store lock was poisoned; recovering");
            poisoned.into_inner()
        }
    };
    let now = Instant::now();
    map.retain(|_, v| v.expires_at > now);
    map.insert(
        user_id.to_string(),
        StoredToken {
            token: token.to_string(),
            expires_at,
        },
    );
}

/// The live token for `user_id`, or None when there is none or it has expired.
fn lookup(user_id: &str) -> Option<(String, u64)> {
    let mut map = match store().lock() {
        Ok(m) => m,
        Err(poisoned) => {
            tracing::warn!(target: "desktop_update", "token store lock was poisoned; recovering");
            poisoned.into_inner()
        }
    };
    let now = Instant::now();
    map.retain(|_, v| v.expires_at > now);
    map.get(user_id)
        .map(|v| (v.token.clone(), v.expires_at.duration_since(now).as_secs()))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/desktop/registry-token", get(get_registry_token))
}

/// `GET /desktop/registry-token` - the registry credential for the caller.
///
/// Session-authenticated: an unauthenticated caller gets 401 and no token. The
/// desktop bridge is the only expected consumer, but nothing about the response
/// is desktop-specific, so no client sniffing is done.
async fn get_registry_token(OptionalUser(user): OptionalUser) -> Response {
    let Some(user) = user else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "sign in to Let's Chat to get an update credential" })),
        )
            .into_response();
    };
    match lookup(&user.id) {
        Some((token, expires_in)) => (
            StatusCode::OK,
            Json(json!({ "token": token, "expires_in": expires_in })),
        )
            .into_response(),
        None => {
            tracing::info!(target: "desktop_update", user_id = %user.id, "no registry token held for this user");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "no update credential is held for this session; sign in again"
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(user_id: &str, token: &str, expires_at: Instant) {
        store().lock().unwrap().insert(
            user_id.to_string(),
            StoredToken {
                token: token.to_string(),
                expires_at,
            },
        );
    }

    #[test]
    fn a_live_token_is_returned_to_its_own_user_only() {
        put(
            "user-live",
            "tok-live",
            Instant::now() + Duration::from_secs(60),
        );
        let (token, expires_in) = lookup("user-live").expect("token stored for this user");
        assert_eq!(token, "tok-live");
        assert!(expires_in > 0 && expires_in <= 60);
        assert!(
            lookup("someone-else").is_none(),
            "a token must never be handed to another user"
        );
    }

    #[test]
    fn an_expired_token_is_dropped_rather_than_served() {
        put(
            "user-expired",
            "tok-expired",
            Instant::now() - Duration::from_secs(1),
        );
        assert!(lookup("user-expired").is_none());
        assert!(
            !store().lock().unwrap().contains_key("user-expired"),
            "expired entries must be pruned, not just hidden"
        );
    }
}
