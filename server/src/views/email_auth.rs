use askama::Template;

#[derive(Template)]
#[template(path = "email/verify_email.txt", escape = "none")]
pub struct VerifyEmailText<'a> {
    pub link: &'a str,
    pub hours: i64,
}

#[derive(Template)]
#[template(path = "email/verify_email.html")]
pub struct VerifyEmailHtml<'a> {
    pub link: &'a str,
    pub hours: i64,
}

#[derive(Template)]
#[template(path = "email/password_reset.txt", escape = "none")]
pub struct PasswordResetText<'a> {
    pub link: &'a str,
    pub ttl: i64,
}

#[derive(Template)]
#[template(path = "email/password_reset.html")]
pub struct PasswordResetHtml<'a> {
    pub link: &'a str,
    pub ttl: i64,
}
