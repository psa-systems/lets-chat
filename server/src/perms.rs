use crate::models::enclave::EnclaveRole;

fn is_site_admin(site_role: &str) -> bool {
    site_role == "admin"
}

pub fn enclave_can_manage(role: Option<EnclaveRole>, site_role: &str) -> bool {
    if is_site_admin(site_role) {
        return true;
    }
    matches!(role, Some(EnclaveRole::Owner | EnclaveRole::Admin))
}

pub fn enclave_can_delete(role: Option<EnclaveRole>, site_role: &str) -> bool {
    if is_site_admin(site_role) {
        return true;
    }
    matches!(role, Some(EnclaveRole::Owner))
}

pub fn enclave_can_invite(role: Option<EnclaveRole>, site_role: &str) -> bool {
    enclave_can_manage(role, site_role)
}

pub fn enclave_can_add_room(role: Option<EnclaveRole>, site_role: &str) -> bool {
    enclave_can_manage(role, site_role)
}

pub fn enclave_can_manage_admins(role: Option<EnclaveRole>, site_role: &str) -> bool {
    if is_site_admin(site_role) {
        return true;
    }
    matches!(role, Some(EnclaveRole::Owner))
}

/// LC-84: who can grant/revoke per-room moderator overrides. Same set
/// as `enclave_can_manage` (site admin, enclave owner, enclave admin);
/// room-level Moderator overrides are themselves an elevation primitive
/// and so must be gated by an admin-tier role.
pub fn room_can_manage_overrides(enclave_role: Option<EnclaveRole>, site_role: &str) -> bool {
    enclave_can_manage(enclave_role, site_role)
}

/// LC-679: the single question every LLM/AI call site asks - the pure
/// combinator behind the runtime, role-scoped gate. The feature is usable only
/// when ALL THREE hold:
/// - `flag_on`: the runtime kill switch is on (settings `llm_enabled`, defaulted
///   OFF in production so the merged feature is dark until an admin flips it);
/// - `configured`: the client is wired (`LlmConfig::from_env` / the embeddings
///   equivalent produced a client) - config presence stays a precondition, not
///   the switch;
/// - `privileged`: the viewer holds a privileged role for the context, folding
///   site admin OR enclave Owner/Admin OR room Moderator (resolved by
///   `routes::ai_gate::privileged_in_room`, which reuses [`enclave_can_manage`]
///   and `room_rbac::is_room_moderator`). A DM has no enclave / room role, so
///   only a site admin qualifies there.
///
/// Keeping the three inputs explicit lets the render path (which computes a UI
/// boolean) and the route guard (which returns 403) ask the identical question.
pub fn can_use_llm(flag_on: bool, configured: bool, privileged: bool) -> bool {
    flag_on && configured && privileged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_use_llm_requires_all_three() {
        assert!(can_use_llm(true, true, true));
        // Any single input off closes the gate.
        assert!(!can_use_llm(false, true, true), "flag off");
        assert!(!can_use_llm(true, false, true), "unconfigured");
        assert!(!can_use_llm(true, true, false), "unprivileged");
    }

    #[test]
    fn enclave_manage_folds_site_admin_and_owner_admin() {
        // Site admin qualifies regardless of enclave role (or none).
        assert!(enclave_can_manage(None, "admin"));
        assert!(enclave_can_manage(Some(EnclaveRole::Member), "admin"));
        // Enclave Owner/Admin qualify without site-admin.
        assert!(enclave_can_manage(Some(EnclaveRole::Owner), "user"));
        assert!(enclave_can_manage(Some(EnclaveRole::Admin), "user"));
        // A plain member / no membership does not.
        assert!(!enclave_can_manage(Some(EnclaveRole::Member), "user"));
        assert!(!enclave_can_manage(None, "user"));
    }
}
