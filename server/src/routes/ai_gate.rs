//! LC-679: runtime, role-scoped gate for the LLM/Ollama + embeddings surface.
//!
//! Before this, the only rollout control was env presence: `LlmConfig::from_env`
//! returned `None` when `LETS_CHAT_LLM_URL` was unset, so turning the feature
//! off meant a redeploy, and once on every user got it. This module adds two
//! layers on top of that env precondition, both asked through the single pure
//! predicate [`crate::perms::can_use_llm`]:
//!
//! 1. A runtime kill switch - the settings row [`LLM_ENABLED_KEY`], defaulted
//!    OFF (absent) in production. It is read live from the `settings` KV store
//!    (the same store as `maintenance_mode` / `link_filter_enabled`), so an
//!    admin toggle takes effect on the next request with no restart. Config
//!    presence stays a precondition, not the switch.
//! 2. Role scoping - when the flag is on, the feature is exposed only to the
//!    site admin role, enclave `Owner`/`Admin`, and room Moderators, reusing the
//!    existing [`crate::perms`] predicates and `db::room_rbac::is_room_moderator`
//!    rather than a new role type. A DM / enclave-less room has no such roles,
//!    so it reduces to site-admin-only.
//!
//! The render path uses [`flag_on`] + [`privileged_in_room`] to compute the UI
//! booleans (so an ungated viewer sees no trace of the feature), and every
//! LLM/embeddings route calls a `require_*` guard so a direct request from an
//! unprivileged user gets a 403 rather than merely a hidden button. The flag is
//! kept after rollout as a kill switch.

use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::perms::{can_use_llm, enclave_can_manage};
use crate::state::AppState;

/// Settings key for the runtime LLM kill switch. Absent - or any value other
/// than `"true"` - means OFF, so a production deployment that never seeds it
/// defaults the whole surface off with no deploy.
pub const LLM_ENABLED_KEY: &str = "llm_enabled";

/// True when the runtime LLM feature flag is ON. Read live from the settings KV
/// store (no cache), so toggling in `/admin/settings` takes effect on the next
/// request without a restart.
pub async fn flag_on(state: &AppState) -> bool {
    db::settings::get_setting(&state.settings, LLM_ENABLED_KEY)
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("true")
}

/// Is `user` privileged to use AI in `room_id`'s context? Site admin OR enclave
/// `Owner`/`Admin` of the room's enclave OR room Moderator. Reuses the existing
/// role model: `is_room_moderator` folds site-admin (via the org-role rank) and
/// per-room Moderator overrides, and [`enclave_can_manage`] folds site-admin and
/// enclave `Owner`/`Admin`. A DM / enclave-less room has no enclave and no
/// override rows, so this naturally reduces to site-admin-only.
pub async fn privileged_in_room(
    state: &AppState,
    room_id: i64,
    user: &User,
) -> Result<bool, AppError> {
    if db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await? {
        return Ok(true);
    }
    if let Some(enclave_id) = db::chat::room_enclave_id(&state.chat, room_id).await? {
        let membership = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
        if enclave_can_manage(membership.map(|m| m.role), &user.role) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Full embeddings gate for a room-context decision: flag ON, client configured,
/// and the viewer privileged. Used by semantic search to decide whether to rank
/// by embeddings (the render paths compute `llm_available` / `embeddings_available`
/// inline from `flag_on` + [`privileged_in_room`] + the relevant `*_available()`).
pub async fn can_use_embeddings_in_room(
    state: &AppState,
    room_id: i64,
    user: &User,
) -> Result<bool, AppError> {
    if !state.embeddings_available() || !flag_on(state).await {
        return Ok(false);
    }
    Ok(can_use_llm(
        true,
        true,
        privileged_in_room(state, room_id, user).await?,
    ))
}

/// Route guard: `Ok(())` when the runtime flag is on AND the viewer is
/// privileged for this room, else [`AppError::Forbidden`] (403). Call at the top
/// of every LLM route so an unprivileged or flag-off request is refused on the
/// server, not just hidden. This deliberately does NOT check config presence -
/// the route keeps its own `let Some(llm) = ... else { ... }` guard, which
/// returns the feature-specific "not configured" message. Splitting the two
/// keeps the "AI unconfigured" and "not allowed" responses distinct.
pub async fn require_llm_in_room(
    state: &AppState,
    room_id: i64,
    user: &User,
) -> Result<(), AppError> {
    if flag_on(state).await && privileged_in_room(state, room_id, user).await? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Route guard for the embeddings surface (find-related). Flag + role only; the
/// route keeps its own config check. (Semantic search does not use this - it
/// degrades to keyword via [`can_use_embeddings_in_room`] rather than 403.)
pub async fn require_embeddings_in_room(
    state: &AppState,
    room_id: i64,
    user: &User,
) -> Result<(), AppError> {
    if flag_on(state).await && privileged_in_room(state, room_id, user).await? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
