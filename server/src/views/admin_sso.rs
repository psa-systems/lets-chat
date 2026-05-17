//! View structs for the `/admin/sso` provider-management UI.
//!
//! Two pages today: the list view (`SsoListPage`) and the
//! create/edit form (`SsoEditPage`). The form is shared between the
//! "Add provider" and "Edit existing" entry points; `editing` flips
//! the few labels and form-action attributes that differ. Test-button
//! results land on the edit page itself via a flash field
//! (`test_result`) to keep the surface small.

use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// Per-row projection for the providers list. Carries the few fields
/// the table renders so callers don't have to thread the full
/// `SsoProviderRow` through.
pub struct AdminSsoProviderView {
    pub id: String,
    pub display_name: String,
    pub issuer_url: String,
    pub enabled: bool,
    pub allow_signup: bool,
    pub linked_users: i64,
}

/// Pre-populated attribute-map fields for the edit form. Five labelled
/// text inputs instead of raw JSON (doc 10 section 2).
#[derive(Debug, Default, Clone)]
pub struct AttributeMapForm {
    pub email_claim: String,
    pub email_verified_claim: String,
    pub name_claim: String,
    pub username_claim: String,
    pub groups_claim: String,
}

/// Result of the "Test" action: discovered endpoints rendered back to
/// the admin so they can confirm the IdP answered correctly before
/// saving.
#[derive(Debug, Clone)]
pub struct DiscoveryTestResult {
    pub ok: bool,
    pub message: String,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub userinfo_endpoint: Option<String>,
    pub jwks_uri: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/sso_list.html")]
pub struct SsoListPage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub providers: &'a [AdminSsoProviderView],
    /// Flash banner after a write. None hides the banner.
    pub flash: Option<&'a str>,
    /// True when the secret key is unset; the "Add provider" button
    /// renders disabled with a tooltip explaining why.
    pub secret_key_missing: bool,
}

#[derive(Template)]
#[template(path = "admin/sso_edit.html")]
pub struct SsoEditPage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// True when the form is editing an existing row; false on create.
    /// Drives the form-action URL and a few labels.
    pub editing: bool,
    pub id: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    pub scopes: String,
    pub allow_signup: bool,
    pub auto_link_verified_email: bool,
    pub attribute_map: AttributeMapForm,
    pub enabled: bool,
    /// `Some` when the user just clicked "Test"; renders an inline
    /// success/failure card with the discovered endpoints.
    pub test_result: Option<DiscoveryTestResult>,
    /// Set when the form was redirected back due to a validation error.
    pub error: Option<String>,
    /// Set when the form just saved successfully.
    pub flash: Option<String>,
}
