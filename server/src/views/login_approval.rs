#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

/// LC-587: plain-text body of the "approve this sign-in" email carrying the
/// single-use 6-digit code.
#[derive(Template)]
#[template(path = "email/login_approval.txt", escape = "none")]
pub struct LoginApprovalText<'a> {
    pub username: &'a str,
    pub code: &'a str,
    pub country: &'a str,
    pub device_label: &'a str,
    pub ip: &'a str,
    pub when: &'a str,
    pub settings_url: &'a str,
}

/// LC-587: HTML body of the "approve this sign-in" email.
#[derive(Template)]
#[template(path = "email/login_approval.html")]
pub struct LoginApprovalHtml<'a> {
    pub username: &'a str,
    pub code: &'a str,
    pub country: &'a str,
    pub device_label: &'a str,
    pub ip: &'a str,
    pub when: &'a str,
    pub settings_url: &'a str,
}

/// LC-587: the interstitial page shown when a login is withheld pending
/// approval. It carries the opaque challenge `token` in a hidden field and
/// prompts for the emailed code; `error` renders a banner on a wrong code.
#[derive(Template)]
#[template(path = "auth/login_approve.html")]
pub struct LoginApprovePage<'a> {
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
    pub token: &'a str,
    pub error: Option<&'a str>,
}
