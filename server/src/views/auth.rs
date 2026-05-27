use askama::Template;

// LC-100: the `| t` / `| tn` i18n filters resolve to `filters::t(..)` in the
// generated template code, so bring the filter module into scope.
#[allow(unused_imports)]
use crate::i18n::filters;

#[derive(Template)]
#[template(path = "auth/login.html")]
pub struct LoginPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
    /// LC-96: True when the global branding row has a logo set; the
    /// template renders `<img src="/branding/logo">` only in that
    /// case to avoid a guaranteed 404 + broken-image icon.
    pub brand_logo: bool,
    /// LC-96: operator-supplied heading shown above the form. Empty
    /// when unset; the template falls back to the default "Sign in".
    pub brand_heading: String,
    /// LC-96: operator-supplied body rendered as restricted markdown
    /// (no code blocks). Already escaped + sanitized at this point.
    /// Empty when unset.
    pub brand_body_html: String,
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
