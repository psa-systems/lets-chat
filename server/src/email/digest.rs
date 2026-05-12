//! Email-digest content primitives (phase 22 task 3).
//!
//! `build_snippet` is the only public entry point in this file today. It
//! takes a raw message body and returns a `(plaintext, html)` pair suitable
//! for inclusion in the digest's two-part `multipart/alternative` body.
//!
//! Future tasks (4, scheduler tick) layer the eligibility query and dispatch
//! loop on top of this module; they reuse `build_snippet` to render each
//! digest item.
//!
//! Design notes:
//! - The digest renders messages MUCH more conservatively than the in-app
//!   message bubble (see `views::room::render_body`). No custom emojis, no
//!   profile-link chips, no avatars: email clients have wildly different
//!   HTML support and the safe set is "escaped text, bold, anchor."
//! - `@username` substrings get `<strong>` treatment in the HTML form
//!   without any resolution lookup. This matches what the author typed
//!   rather than what the mention actually pointed at; for digest copy that
//!   is the right surface (the recipient sees the same prefix they would
//!   in the app's notification toast).
//! - URLs are linkified in the HTML form only. The plaintext form leaves
//!   them as literal text so mail clients with conservative URL detection
//!   still click through them correctly.

/// Render a one-message snippet for inclusion in a digest item.
///
/// Returns `(plain, html)`:
///   * `plain`: trimmed, word-truncated to ~140 chars, ASCII ellipsis when
///     truncated. No escaping.
///   * `html`: same truncation, HTML-escaped, then `@name` substrings
///     wrapped in `<strong>` and URLs wrapped in anchor tags.
///
/// The 140-char cap is the same shape used by Push payloads and the in-app
/// snippet (phase 16). Word-boundary truncation avoids "...take a l" mid-word
/// breaks; the loop accumulates whitespace-separated chunks while their
/// combined length plus the next chunk fits.
pub fn build_snippet(body: &str) -> (String, String) {
    const MAX_CHARS: usize = 140;
    let trimmed = body.trim();
    let (truncated, truncated_did) = word_truncate(trimmed, MAX_CHARS);
    let plain = if truncated_did {
        format!("{truncated}...")
    } else {
        truncated.to_string()
    };
    let html_inner = render_html_snippet(truncated);
    let html = if truncated_did {
        format!("{html_inner}...")
    } else {
        html_inner
    };
    (plain, html)
}

/// Truncate `s` at a whitespace boundary so the result fits within
/// `max_chars` UTF-8 bytes (with room for a trailing "..." that the
/// caller appends). Returns `(slice, was_truncated)` where `slice` is a
/// sub-slice of the input, never split mid-grapheme because the cut is
/// always at an ASCII whitespace byte.
fn word_truncate(s: &str, max_chars: usize) -> (&str, bool) {
    if s.len() <= max_chars {
        return (s, false);
    }
    // Walk byte indices at whitespace boundaries from the front; keep the
    // last boundary that still fits in `max_chars`.
    let mut last_fit = 0usize;
    let mut chunks = s.split_inclusive(char::is_whitespace);
    let mut acc = 0usize;
    for chunk in &mut chunks {
        let next = acc + chunk.len();
        if next > max_chars {
            break;
        }
        acc = next;
        last_fit = acc;
    }
    // `last_fit` may have a trailing whitespace from `split_inclusive`;
    // trim it off so the rendered "...":
    //   "Hello world ..." -> "Hello world..."
    let cut = s[..last_fit].trim_end();
    if cut.is_empty() {
        // A single 200-char word with no whitespace cannot be word-broken.
        // Fall back to a hard cut at a char boundary <= max_chars.
        let mut i = max_chars;
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        (&s[..i], true)
    } else {
        (cut, true)
    }
}

fn render_html_snippet(s: &str) -> String {
    use std::sync::OnceLock;
    static MENTION_RE: OnceLock<regex::Regex> = OnceLock::new();
    // Same shape as `db::mentions::TOKEN_PATTERN`: leading whitespace OR
    // start-of-string, then `@` + 1-32 chars of `[A-Za-z0-9_-]`. Captures
    // the username so we can wrap it in <strong> without including the
    // leading whitespace inside the tag.
    let mention_re = MENTION_RE
        .get_or_init(|| regex::Regex::new(r"(?:^|\s)@([A-Za-z0-9_-]{1,32})").expect("regex"));

    // Pass 1: linkify URL runs. Operates on the raw (pre-escape) text so
    // the `LinkFinder` sees the URLs as the user typed them.
    let finder = linkify::LinkFinder::new();
    let mut out = String::with_capacity(s.len() + 32);
    let mut cursor = 0usize;
    for link in finder.links(s) {
        if !matches!(link.kind(), linkify::LinkKind::Url) {
            continue;
        }
        let start = link.start();
        let end = link.end();
        if start > cursor {
            out.push_str(&apply_mention_bold(
                &html_escape(&s[cursor..start]),
                mention_re,
            ));
        }
        let url = link.as_str();
        out.push_str("<a href=\"");
        out.push_str(&html_escape(url));
        out.push_str("\">");
        out.push_str(&html_escape(url));
        out.push_str("</a>");
        cursor = end;
    }
    if cursor < s.len() {
        out.push_str(&apply_mention_bold(&html_escape(&s[cursor..]), mention_re));
    }
    out
}

/// Wrap each `@name` substring in `<strong>@name</strong>`. Operates on
/// already-HTML-escaped text so the regex matches the literal `@` followed
/// by the entity-free username (escaping does not affect `[A-Za-z0-9_-]`).
fn apply_mention_bold(escaped: &str, re: &regex::Regex) -> String {
    let mut out = String::with_capacity(escaped.len() + 16);
    let mut cursor = 0usize;
    for cap in re.captures_iter(escaped) {
        let whole = cap.get(0).unwrap();
        let name = cap.get(1).unwrap();
        // The leading boundary (whitespace or BOS) lives at whole.start();
        // the `@` starts one byte before name.start(). Copy everything up
        // to the `@` verbatim so the boundary character is preserved.
        let at_pos = name.start() - 1;
        if at_pos > cursor {
            out.push_str(&escaped[cursor..at_pos]);
        }
        out.push_str("<strong>@");
        out.push_str(name.as_str());
        out.push_str("</strong>");
        cursor = whole.end();
    }
    if cursor < escaped.len() {
        out.push_str(&escaped[cursor..]);
    }
    out
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
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_round_trips_unchanged() {
        let (plain, html) = build_snippet("Hello world");
        assert_eq!(plain, "Hello world");
        assert_eq!(html, "Hello world");
    }

    #[test]
    fn trim_strips_leading_and_trailing_whitespace() {
        let (plain, _) = build_snippet("   spaced out   ");
        assert_eq!(plain, "spaced out");
    }

    #[test]
    fn long_string_word_truncates_with_ellipsis() {
        // Build a body comfortably over the 140-char cap so truncation
        // must engage. Each repeat of the chunk is 23 chars (incl. trailing
        // space); 10 repeats = 230 chars.
        let body = "alpha beta gamma delta ".repeat(10);
        assert!(body.len() > 140, "test setup: body must exceed cap");
        let (plain, html) = build_snippet(&body);
        assert!(
            plain.ends_with("..."),
            "plaintext should end with ellipsis; got {plain:?}"
        );
        assert!(
            html.ends_with("..."),
            "html should end with ellipsis; got {html:?}"
        );
        // Word-truncate: the last word before "..." must be a complete
        // token from the input vocabulary, never a partial slice.
        let plain_no_dots = plain.trim_end_matches('.');
        let last_word = plain_no_dots.split_whitespace().next_back().unwrap();
        assert!(
            ["alpha", "beta", "gamma", "delta"].contains(&last_word),
            "last word {last_word:?} should be one of alpha/beta/gamma/delta"
        );
        // Length cap: the truncated content (before "...") must fit
        // within MAX_CHARS.
        assert!(
            plain_no_dots.len() <= 140,
            "plain still over cap: {} chars",
            plain_no_dots.len()
        );
    }

    #[test]
    fn html_escapes_special_characters_but_plain_leaves_them() {
        let body = r#"a & b <c> "d" 'e'"#;
        let (plain, html) = build_snippet(body);
        assert_eq!(plain, r#"a & b <c> "d" 'e'"#);
        assert!(html.contains("&amp;"));
        assert!(html.contains("&lt;c&gt;"));
        assert!(html.contains("&quot;d&quot;"));
        assert!(html.contains("&#39;e&#39;"));
        // The literal chars must NOT appear unescaped.
        assert!(!html.contains(" & "), "html still has unescaped ampersand");
        assert!(
            !html.contains("<c>"),
            "html still has unescaped angle bracket"
        );
    }

    #[test]
    fn mentions_bolded_in_html_only() {
        let body = "Hey @alice and @bob_42-x can you take a look?";
        let (plain, html) = build_snippet(body);
        assert_eq!(plain, "Hey @alice and @bob_42-x can you take a look?");
        assert!(
            html.contains("<strong>@alice</strong>"),
            "expected @alice bolded; got {html:?}"
        );
        assert!(
            html.contains("<strong>@bob_42-x</strong>"),
            "expected @bob_42-x bolded; got {html:?}"
        );
    }

    #[test]
    fn email_address_is_not_treated_as_mention() {
        // Same boundary rule as the mention parser: `@` must be preceded
        // by whitespace or start-of-string, so `foo@bar.com` should NOT
        // produce a `<strong>@bar</strong>` chip.
        let body = "Mail me at foo@bar.com please.";
        let (_, html) = build_snippet(body);
        assert!(
            !html.contains("<strong>"),
            "email-style @ should not produce a mention chip; got {html:?}"
        );
    }

    #[test]
    fn urls_linkified_in_html_only() {
        let body = "See https://example.com/path?q=1 for details";
        let (plain, html) = build_snippet(body);
        assert_eq!(plain, "See https://example.com/path?q=1 for details");
        assert!(
            html.contains(r#"<a href="https://example.com/path?q=1">"#),
            "expected url anchor; got {html:?}"
        );
    }

    #[test]
    fn mention_then_url_both_render() {
        let body = "Hey @alice look at https://example.com";
        let (_, html) = build_snippet(body);
        assert!(html.contains("<strong>@alice</strong>"));
        assert!(html.contains(r#"<a href="https://example.com">"#));
    }

    #[test]
    fn single_long_word_falls_back_to_hard_cut() {
        // A single 200-char "word" has no whitespace to break at. The
        // helper should still cap length rather than emit unbounded text.
        let body = "x".repeat(200);
        let (plain, _) = build_snippet(&body);
        assert!(plain.ends_with("..."));
        // Body of 'x' chars plus 3 dots; the cut respects char boundaries
        // and stays at-or-under the cap.
        assert!(plain.len() <= 140 + 3, "got len {}: {plain:?}", plain.len());
    }

    #[test]
    fn no_break_inside_url_html_treats_url_as_atomic_segment() {
        // Even with mention bolding active, the URL anchor must stay
        // intact (no <strong> wrapping inside the href text).
        let body = "@alice https://example.com/about";
        let (_, html) = build_snippet(body);
        assert!(html.contains(r#"<a href="https://example.com/about">"#));
        // The URL contents inside the anchor should NOT be mention-bolded:
        // it has no @ but if a future URL contained @ we would rely on this
        // separation. Sanity check: only one <strong> in the output (the
        // leading @alice), not a stray one inside the URL.
        assert_eq!(
            html.matches("<strong>").count(),
            1,
            "exactly one mention chip expected: {html:?}"
        );
    }
}
