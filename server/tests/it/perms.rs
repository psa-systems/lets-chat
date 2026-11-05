use lets_chat::models::enclave::EnclaveRole;
use lets_chat::perms::{
    enclave_can_add_room, enclave_can_delete, enclave_can_invite, enclave_can_manage,
    enclave_can_manage_admins,
};

#[test]
fn site_admin_godmode_short_circuits_every_check() {
    for f in [
        enclave_can_manage,
        enclave_can_delete,
        enclave_can_invite,
        enclave_can_add_room,
        enclave_can_manage_admins,
    ] {
        assert!(f(None, "admin"));
        assert!(f(Some(EnclaveRole::Member), "admin"));
    }
}

#[test]
fn member_only_reads() {
    let r = Some(EnclaveRole::Member);
    assert!(!enclave_can_manage(r, "user"));
    assert!(!enclave_can_delete(r, "user"));
    assert!(!enclave_can_invite(r, "user"));
    assert!(!enclave_can_add_room(r, "user"));
    assert!(!enclave_can_manage_admins(r, "user"));
}

#[test]
fn admin_can_manage_invite_addroom_but_not_delete_or_admin_mgmt() {
    let r = Some(EnclaveRole::Admin);
    assert!(enclave_can_manage(r, "user"));
    assert!(enclave_can_invite(r, "user"));
    assert!(enclave_can_add_room(r, "user"));
    assert!(!enclave_can_delete(r, "user"));
    assert!(!enclave_can_manage_admins(r, "user"));
}

#[test]
fn owner_can_do_everything() {
    let r = Some(EnclaveRole::Owner);
    assert!(enclave_can_manage(r, "user"));
    assert!(enclave_can_invite(r, "user"));
    assert!(enclave_can_add_room(r, "user"));
    assert!(enclave_can_delete(r, "user"));
    assert!(enclave_can_manage_admins(r, "user"));
}

#[test]
fn no_membership_no_powers() {
    assert!(!enclave_can_manage(None, "user"));
    assert!(!enclave_can_delete(None, "user"));
}
