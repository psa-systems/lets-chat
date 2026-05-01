use askama::Template;

#[derive(Template)]
#[template(path = "auth/login.html")]
pub struct LoginPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "auth/register.html")]
pub struct RegisterPage<'a> {
    pub error: Option<&'a str>,
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "auth/form_errors.html")]
pub struct FormErrors<'a> {
    pub error: Option<&'a str>,
}
