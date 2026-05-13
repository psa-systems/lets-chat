use askama::Template;

#[derive(Template)]
#[template(path = "email/login_alert.txt", escape = "none")]
pub struct LoginAlertText<'a> {
    pub username: &'a str,
    pub device_label: &'a str,
    pub ip: &'a str,
    pub when: &'a str,
    pub sessions_url: &'a str,
    pub settings_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/login_alert.html")]
pub struct LoginAlertHtml<'a> {
    pub username: &'a str,
    pub device_label: &'a str,
    pub ip: &'a str,
    pub when: &'a str,
    pub sessions_url: &'a str,
    pub settings_url: &'a str,
}
