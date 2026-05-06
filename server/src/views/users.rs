use askama::Template;

#[derive(Template)]
#[template(path = "users/search.html")]
pub struct ProfileSearchFragment<'a> {
    pub query: &'a str,
    pub results: &'a [ProfileResult],
}

pub struct ProfileResult {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub bio: Option<String>,
    pub is_self: bool,
}

impl ProfileResult {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
}
