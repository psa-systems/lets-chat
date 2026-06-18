use askama::Template;

// LC-100: the `| t` / `| tn` i18n filters resolve to `filters::t(..)` in the
// generated template code, so bring the filter module into scope.
#[allow(unused_imports)]
use crate::i18n::filters;

/// LC-22 cutover: the login page is the "Sign in with Bunyip" shell. No form,
/// no fields. Existing branding fields stay so operators retain their custom
/// logo / heading / body markdown over the SSO button.
#[derive(Template)]
#[template(path = "auth/login.html")]
pub struct LoginPage<'a> {
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
    pub brand_logo: bool,
    pub brand_heading: String,
    pub brand_body_html: String,
    /// LC-22: optional `?sso_error=<reason>` query parameter rendered as a
    /// brief banner above the button when an earlier dance failed.
    pub sso_error: Option<&'a str>,
    /// LC-22: host portion of the configured Bunyip issuer, rendered in the
    /// "you will be redirected to ..." note under the button.
    pub bunyip_issuer_host: &'a str,
}
