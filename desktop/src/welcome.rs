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
// Any 2xx/3xx response (including the login redirect a fresh lets-chat
// returns on `/`) counts as reachable. ureq treats 4xx/5xx as errors via
// its `Error::Status` variant, which we map back to "reachable" too - the
// server is up and the desktop client will get the same response the
// webview will. Only network-layer failures (DNS, connect, TLS, timeout)
// short-circuit to the welcome page.
pub fn server_reachable(url: &str) -> Result<(), String> {
    match ureq::get(url).timeout(PROBE_TIMEOUT).call() {
        Ok(_) => Ok(()),
        // ureq returns Status for HTTP-level errors; the server answered,
        // so the webview will see the same response. Not our problem to
        // intercept.
        Err(ureq::Error::Status(_, _)) => Ok(()),
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
