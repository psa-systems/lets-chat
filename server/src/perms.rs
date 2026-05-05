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
