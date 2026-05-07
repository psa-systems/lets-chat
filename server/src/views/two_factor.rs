use askama::Template;

#[derive(Template)]
#[template(path = "two_factor/setup.html")]
pub struct TwoFactorSetupPage<'a> {
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
