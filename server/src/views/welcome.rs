use askama::Template;

// LC-100: the `| t` i18n filter resolves to `filters::t(..)` in the generated
// template code, so bring the filter module into scope.
#[allow(unused_imports)]
use crate::i18n::filters;

/// LC-766: first-entry handle prompt. Shown to a newly provisioned user whose
/// handle is still the derived value (`username_confirmed_at IS NULL`). The
/// field is pre-filled with the derived handle so accepting it is one click;
/// editing it picks a deliberate handle instead. The gate redirects every other
/// authenticated page here until the user confirms.
#[derive(Template)]
#[template(path = "auth/welcome_handle.html")]
pub struct WelcomeHandlePage<'a> {
    pub asset_version: &'a str,
    /// The current (derived) handle, pre-filled into the field.
    pub handle: &'a str,
    /// A validation message from a rejected submit, or None on first render.
    pub error: Option<&'a str>,
    pub brand_logo: bool,
    pub brand_heading: String,
    /// LC-864: the environment this newly provisioned account lives on
    /// (staging, dev, ...), or None in production. Stated on the first-entry
    /// prompt so a new user knows which environment they were provisioned on.
    pub environment: Option<&'a str>,
}
