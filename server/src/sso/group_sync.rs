//! Project the id_token's groups claim onto enclave memberships per
//! the configured `sso_group_mappings` rows for a provider. Called
//! after a successful SSO sign-in.
//!
//! Rules (doc 10 section 3):
//!   1. Compute intended memberships: for every group the IdP listed
//!      for the user, look up the mappings table and gather the
//!      (enclave_id, role) pairs. When the same enclave appears under
//!      multiple groups, the higher role wins.
//!   2. Compute the "scoped" set: every enclave_id that any mapping
//!      for this provider references. Memberships outside that set
//!      are untouched (operators may grant access manually independent
//!      of SSO; the sync only governs enclaves explicitly listed in
//!      mappings).
//!   3. For each scoped enclave: insert / update / remove the user's
//!      membership to match the intended state.
//!   4. Owners are never demoted. If the current role is `owner`, the
//!      sync skips the row entirely - operators promote/demote owners
//!      through the admin UI, not via SSO claims.

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::db::enclave;
use crate::db::sso_group_mappings as gm;
use crate::models::enclave::EnclaveRole;

#[derive(Debug, thiserror::Error)]
pub enum GroupSyncError {
    #[error("database error during group sync: {0}")]
    Db(#[from] sqlx::Error),
}

/// Map the L16 schema's role strings onto the chat-side `EnclaveRole`
/// enum. The L16 schema uses `User` / `Moderator` / `Admin`; the chat
/// side has `Owner` / `Admin` / `Member`. There's no `Moderator` on
/// the chat side, so both `Moderator` and `Admin` land as
/// `EnclaveRole::Admin`; `User` is `EnclaveRole::Member`.
fn mapping_role_to_enclave_role(s: &str) -> EnclaveRole {
    match s {
        "Admin" | "Moderator" => EnclaveRole::Admin,
        // "User" + anything unrecognised (defensive default)
        _ => EnclaveRole::Member,
    }
}

/// Numeric rank for "max role wins" when one user lands in multiple
/// groups that all grant access to the same enclave.
fn rank(role: EnclaveRole) -> u8 {
    match role {
        EnclaveRole::Owner => 3,
        EnclaveRole::Admin => 2,
        EnclaveRole::Member => 1,
    }
}

/// Reconcile `user_id`'s enclave memberships against the configured
/// group mappings for `provider_id`, given the IdP-emitted `groups`
/// list. A no-op when the provider has no mappings configured.
pub async fn apply(
    auth: &SqlitePool,
    chat: &SqlitePool,
    user_id: &str,
    provider_id: &str,
    groups: &[String],
) -> Result<(), GroupSyncError> {
    let scoped_rows = gm::list_for_provider(auth, provider_id).await?;
    if scoped_rows.is_empty() {
        return Ok(());
    }
    let scoped: HashSet<i64> = scoped_rows.iter().map(|r| r.enclave_id).collect();

    // Compute intended (enclave_id -> highest-rank role) from the IdP
    // group list intersected with the mappings table.
    let mut intended: HashMap<i64, EnclaveRole> = HashMap::new();
    for group in groups {
        let rows = gm::list_for_group(auth, provider_id, group).await?;
        for r in rows {
            let target = mapping_role_to_enclave_role(&r.role);
            intended
                .entry(r.enclave_id)
                .and_modify(|cur| {
                    if rank(target) > rank(*cur) {
                        *cur = target;
                    }
                })
                .or_insert(target);
        }
    }

    for enclave_id in &scoped {
        let current = enclave::get_membership(chat, *enclave_id, user_id).await?;
        let current_role = current.as_ref().map(|m| m.role);
        let target = intended.get(enclave_id).copied();
        if matches!(current_role, Some(EnclaveRole::Owner)) {
            // Never let SSO claims demote an owner.
            continue;
        }
        match (current_role, target) {
            (None, None) | (Some(EnclaveRole::Owner), _) => {}
            (None, Some(t)) => {
                enclave::add_member(chat, *enclave_id, user_id, t).await?;
            }
            (Some(_), None) => {
                enclave::remove_member(chat, *enclave_id, user_id).await?;
            }
            (Some(c), Some(t)) if c == t => {}
            (Some(_), Some(t)) => {
                enclave::update_role(chat, *enclave_id, user_id, t).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_mapping_lifts_moderator_into_admin() {
        assert_eq!(
            mapping_role_to_enclave_role("Moderator"),
            EnclaveRole::Admin
        );
        assert_eq!(mapping_role_to_enclave_role("Admin"), EnclaveRole::Admin);
        assert_eq!(mapping_role_to_enclave_role("User"), EnclaveRole::Member);
        assert_eq!(mapping_role_to_enclave_role("ghost"), EnclaveRole::Member);
    }

    #[test]
    fn rank_orders_owner_above_admin_above_member() {
        assert!(rank(EnclaveRole::Owner) > rank(EnclaveRole::Admin));
        assert!(rank(EnclaveRole::Admin) > rank(EnclaveRole::Member));
    }
}
