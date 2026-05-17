use askama::Template;

#[derive(Template)]
#[template(path = "auth/login.html")]
pub struct LoginPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/register.html")]
pub struct RegisterPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/form_errors.html")]
pub struct FormErrors<'a> {
    pub error: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "auth/sso_link_required.html")]
pub struct SsoLinkRequiredPage<'a> {
    pub provider_display_name: &'a str,
    pub email: &'a str,
    pub envelope: &'a str,
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/sso_unauthorized.html")]
pub struct SsoUnauthorizedPage<'a> {
    pub provider_display_name: &'a str,
    pub email: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/sso_email_unverified.html")]
pub struct SsoEmailUnverifiedPage<'a> {
    pub provider_display_name: &'a str,
    pub email: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/forgot.html")]
pub struct ForgotPage<'a> {
    pub error: Option<&'a str>,
    pub notice: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/reset.html")]
pub struct ResetPage<'a> {
    pub token: &'a str,
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}

#[derive(Template)]
#[template(path = "auth/verify_email_result.html")]
pub struct VerifyEmailResultPage<'a> {
    pub notice: Option<&'a str>,
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
}
