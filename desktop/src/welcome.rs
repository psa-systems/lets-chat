// Embedded welcome / fallback page for the desktop wrapper.
//
// Shown when the configured `LETS_CHAT_SERVER_URL` does not respond at
// startup. Without this page Wry just renders a blank window (no error, no
// hint), which is what users actually see when the server is down or
// unconfigured. The page tells them which URL failed, why, and how to
// recover.

use std::time::Duration;

const TEMPLATE: &str = include_str!("welcome.html");
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

// Quick HEAD-equivalent probe: GETs the configured URL with a tight timeout.
// Only a 2xx/3xx response on `/` (the lets-chat root redirects to /login)
// counts as reachable. ureq treats 4xx/5xx as errors via its
// `Error::Status` variant; we treat those as unreachable too, because the
// root of a real lets-chat server never 404s. The common case caught here
// is `LETS_CHAT_SERVER_URL` pointing at a port serving a different app
// (whose Problem+JSON 404 would otherwise render as raw JSON in the
// webview). Network-layer failures (DNS, connect, TLS, timeout) also
// short-circuit to the welcome page.
pub fn server_reachable(url: &str) -> Result<(), String> {
    // LC-210: this is the ONE deliberately-unguarded ureq call in the desktop
    // crate (allow-listed in the net_guard grep-ban). It probes the
    // operator-configured LETS_CHAT_SERVER_URL, which is legitimately often a
    // LAN / private IP for self-hosted deployments, and the webview loads that
    // exact URL regardless - so the probe adds no SSRF surface beyond what the
    // app already does. Routing it through net_guard's public-IP filter would
    // break every LAN deployment. Do NOT "harden" this to net_guard without
    // reading that reasoning; the grep-ban meta-test pins this exception.
    match ureq::get(url).timeout(PROBE_TIMEOUT).call() {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code} from {url}")),
        Err(ureq::Error::Transport(t)) => Err(format!("{t}")),
    }
}

// Render the welcome page with the failing URL and a short reason
// substituted in. Plain string replacement so we don't pull in a template
// engine for one page; the only placeholders are produced by this code and
// the values are HTML-escaped here.
pub fn render(url: &str, reason: &str, version: &str, git_hash: &str) -> String {
    TEMPLATE
        .replace("{{URL}}", &html_escape(url))
        .replace("{{REASON}}", &html_escape(reason))
        .replace("{{VERSION}}", &html_escape(version))
        .replace("{{GIT_HASH}}", &html_escape(git_hash))
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
