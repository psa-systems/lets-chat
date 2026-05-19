//! URL extraction and link-filter pattern matching (LC-94).
//!
//! Two responsibilities split into two free functions so the
//! message-send handler and the link-filter unit tests can use them
//! independently:
//!
//! - [`extract_hosts`] walks a message body with `linkify`, parses each
//!   discovered URL with `url::Url`, and returns the lowercased host
//!   strings. Bare hostnames (no scheme) are not extracted - this
//!   matches what `linkify_body` already renders, so the filter
//!   catches the same URLs the user would see as clickable links.
//!
//! - [`pattern_matches`] decides whether a single pattern from
//!   `link_filter_rules` catches a given host. Patterns are either
//!   literal domains (`spammy.com`) or simple globs where `*` is the
//!   wildcard (`*.spammy.com`, `*.tk`, `bad-*-host*`). The matcher
//!   converts the glob to a regex anchored on both sides; this is
//!   one regex compilation per check, which is fine for the volumes
//!   a self-hosted deployment sees.

use linkify::{LinkFinder, LinkKind};
use regex::Regex;
use url::Url;

/// Lower-cased hosts of every URL `linkify` finds in `body`.
/// Order-preserving but de-duplicated, so the link filter checks each
/// host at most once per message.
pub fn extract_hosts(body: &str) -> Vec<String> {
    let finder = LinkFinder::new();
    let mut out: Vec<String> = Vec::new();
    for link in finder.links(body) {
        if !matches!(link.kind(), LinkKind::Url) {
            continue;
        }
        let Ok(parsed) = Url::parse(link.as_str()) else {
            continue;
        };
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let lower = host.to_ascii_lowercase();
        if !out.iter().any(|h| h == &lower) {
            out.push(lower);
        }
    }
    out
}

/// True when `pattern` (a literal host or `*`-glob) matches `host`.
/// Matching is case-insensitive on both sides. An unparseable pattern
/// is treated as a non-match rather than panicking, so a typo'd rule
/// silently does nothing instead of taking the whole filter offline.
pub fn pattern_matches(pattern: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    if !pattern.contains('*') {
        return pattern == host;
    }
    let mut re = String::with_capacity(pattern.len() + 4);
    re.push('^');
    for c in pattern.chars() {
        match c {
            '*' => re.push_str(".*"),
            c if c.is_ascii_alphanumeric() || c == '-' => re.push(c),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).map(|r| r.is_match(&host)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_hosts_lowercased_and_deduped() {
        let body = "see https://Example.com and http://example.com/path and https://other.net";
        let hosts = extract_hosts(body);
        assert_eq!(hosts, vec!["example.com", "other.net"]);
    }

    #[test]
    fn skips_bare_hostnames_without_scheme() {
        // linkify only emits URL-kind links for actual URLs with a
        // scheme or a recognized prefix; bare hostnames don't
        // qualify, which mirrors how the room view renders them.
        let body = "ping example.com please";
        assert!(extract_hosts(body).is_empty());
    }

    #[test]
    fn literal_pattern_exact_match() {
        assert!(pattern_matches("spammy.com", "spammy.com"));
        assert!(!pattern_matches("spammy.com", "sub.spammy.com"));
        assert!(!pattern_matches("spammy.com", "spammy.co"));
    }

    #[test]
    fn glob_leading_star() {
        assert!(pattern_matches("*.tk", "foo.tk"));
        assert!(pattern_matches("*.tk", "a.b.tk"));
        assert!(!pattern_matches("*.tk", "tk"));
        assert!(!pattern_matches("*.tk", "nottk"));
    }

    #[test]
    fn glob_middle_star() {
        assert!(pattern_matches("bad-*-host", "bad-anything-host"));
        assert!(!pattern_matches("bad-*-host", "bad-host"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(pattern_matches("EXAMPLE.com", "example.COM"));
        assert!(pattern_matches("*.TK", "foo.tk"));
    }

    #[test]
    fn unparseable_pattern_does_not_match() {
        // The escaping branch handles weird chars safely, so this is
        // really a guard against future regressions if the matcher
        // ever takes raw regex input.
        assert!(!pattern_matches("(", "anything"));
    }
}
