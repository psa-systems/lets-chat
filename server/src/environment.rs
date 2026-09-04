//! LC-864: the deployment-environment indicator.
//!
//! The team dogfoods on staging, which carries more functionality than
//! production, and nothing in the UI distinguished the two - people could not
//! tell which environment they were in, or which one their account was
//! provisioned against. The label comes from configuration
//! (`LETS_CHAT_ENVIRONMENT`), never inferred from the hostname, so a single
//! deployment declares what it is.
//!
//! Production is the default and shows nothing: an unset, empty, or
//! `production`/`prod` value (any case) normalizes to `None`. Any other value
//! (e.g. `staging`, `dev`) is the label shown in the shell badge and on the
//! sign-in / first-entry / invitation surfaces.

use std::sync::OnceLock;

/// Normalize a raw `LETS_CHAT_ENVIRONMENT` value to the label to display, or
/// `None` when this is production (the default) and nothing should be shown.
/// Trims surrounding whitespace; treats empty and `production` / `prod` (any
/// case) as production. Pure, so the badge logic is unit-tested without touching
/// process state.
pub fn normalize_environment(raw: Option<&str>) -> Option<String> {
    let v = raw?.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("production") || v.eq_ignore_ascii_case("prod") {
        return None;
    }
    Some(v.to_string())
}

/// The configured deployment environment label, or `None` for production. Read
/// once from `LETS_CHAT_ENVIRONMENT` and cached for the life of the process (the
/// deployment cannot change environment while running).
pub fn deployment_environment() -> Option<&'static str> {
    static ENV: OnceLock<Option<String>> = OnceLock::new();
    ENV.get_or_init(|| {
        normalize_environment(std::env::var("LETS_CHAT_ENVIRONMENT").ok().as_deref())
    })
    .as_deref()
}

/// Minimal HTML-attribute/text escaping for the label. The value is
/// operator-controlled configuration, not user input, but the shell badge is
/// spliced into every page by `inject_branding_css`, so escape defensively
/// (matching that middleware's "trust nothing on the write-out path" ethos).
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The deployment-environment badge markup for a given (already normalized)
/// label, or an empty string in production (`None`). Injected into every full
/// page by the shell middleware; production injects nothing. `.lc-env-badge` is
/// styled in main.css (fixed, non-interactive, amber).
pub fn env_badge_html(label: Option<&str>) -> String {
    let label = match label {
        Some(l) if !l.trim().is_empty() => l.trim(),
        _ => return String::new(),
    };
    let esc = escape(label);
    format!(
        "<div class=\"lc-env-badge\" role=\"status\" aria-label=\"{esc} environment\" \
         title=\"You are on the {esc} environment\">{esc}</div>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_normalizes_to_none() {
        assert_eq!(normalize_environment(None), None);
        assert_eq!(normalize_environment(Some("")), None);
        assert_eq!(normalize_environment(Some("   ")), None);
        assert_eq!(normalize_environment(Some("production")), None);
        assert_eq!(normalize_environment(Some("Production")), None);
        assert_eq!(normalize_environment(Some("PROD")), None);
    }

    #[test]
    fn non_production_keeps_the_trimmed_label() {
        assert_eq!(
            normalize_environment(Some("staging")).as_deref(),
            Some("staging")
        );
        assert_eq!(
            normalize_environment(Some("  dev  ")).as_deref(),
            Some("dev")
        );
    }

    #[test]
    fn badge_renders_in_a_non_production_build() {
        let html = env_badge_html(Some("staging"));
        assert!(html.contains("lc-env-badge"), "carries the badge class");
        assert!(html.contains(">staging<"), "shows the environment label");
    }

    #[test]
    fn no_badge_in_production() {
        // Whether we start from a raw "production" value or an unset one, the
        // badge is empty: production shows nothing.
        assert_eq!(
            env_badge_html(normalize_environment(Some("production")).as_deref()),
            ""
        );
        assert_eq!(env_badge_html(normalize_environment(None).as_deref()), "");
        assert_eq!(env_badge_html(None), "");
    }

    #[test]
    fn badge_escapes_the_label() {
        let html = env_badge_html(Some("a<b\"&"));
        assert!(!html.contains("a<b\"&"));
        assert!(html.contains("a&lt;b&quot;&amp;"));
    }
}
