#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

#[derive(Template)]
#[template(path = "two_factor/setup.html")]
pub struct TwoFactorSetupPage<'a> {
    pub username: &'a str,
    pub qr_base64: &'a str,
    pub secret_b32: &'a str,
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
}

/// Same QR + code form as [`TwoFactorSetupPage`], but rendered during the
/// pre-account registration flow. The action posts to `/register/2fa`
/// (which materializes the user on success) and the bail-out link returns
/// to the public register page rather than logout.
#[derive(Template)]
#[template(path = "two_factor/register_setup.html")]
pub struct RegisterTwoFactorSetupPage<'a> {
    pub username: &'a str,
    pub qr_base64: &'a str,
    pub secret_b32: &'a str,
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "two_factor/confirm.html")]
pub struct TwoFactorConfirmPage<'a> {
    pub codes: &'a [String],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "two_factor/login.html")]
pub struct LoginTwoFactorPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "two_factor/recovery.html")]
pub struct LoginRecoveryPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
}
