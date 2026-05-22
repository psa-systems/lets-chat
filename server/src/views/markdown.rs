//! Markdown rendering for message bodies.
//!
//! Wraps `pulldown_cmark` to render CommonMark with a few GFM extensions
//! (strikethrough, tables, task lists). Fenced code blocks are syntax-
//! highlighted server-side via `syntect` and emitted with inline styles so
//! no extra CSS is required.
//!
//! Inline text segments still flow through the existing chat pipeline in
//! `views::room::render_body`: `@username` chips, custom emoji shortcodes,
//! and bare-URL linkification. Raw HTML in user input is dropped so messages
//! can't smuggle markup.
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::db::mentions::MentionRef;
use crate::models::custom_emoji::EmojiRef;
use crate::views::room::{emojify_and_escape, html_escape, render_body};

/// Cache of (body, mentions, emojis) -> rendered HTML. Markdown rendering
/// is fully deterministic in its inputs, and the same message body gets
/// rendered once per viewer when an event is broadcast over the hub;
/// caching collapses that into a single render plus N cheap HashMap
/// lookups. Cache is bounded; on overflow we clear the whole map rather
/// than evict entries individually, which avoids a per-call eviction
/// strategy at the cost of a periodic cold cache.
const MARKDOWN_CACHE_MAX_ENTRIES: usize = 4096;
static MARKDOWN_CACHE: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();

fn markdown_cache_key(body: &str, mentions: &[MentionRef], emojis: &[EmojiRef]) -> u64 {
    let mut hasher = DefaultHasher::new();
    body.hash(&mut hasher);
    for m in mentions {
        m.user_id.hash(&mut hasher);
        m.username.hash(&mut hasher);
    }
    for e in emojis {
        e.shortcode.hash(&mut hasher);
        e.id.hash(&mut hasher);
    }
    hasher.finish()
}

/// Render a message body to HTML.
///
/// Mentions and custom emojis are processed only on inline text segments -
/// they are not interpreted inside fenced code blocks or inline `code`
/// spans, where the literal text is preserved.
pub fn render(body: &str, mentions: &[MentionRef], emojis: &[EmojiRef]) -> String {
    let key = markdown_cache_key(body, mentions, emojis);
    let cache = MARKDOWN_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.get(&key) {
            return cached.clone();
        }
    }

    let rendered = render_inner(body, mentions, emojis);

    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= MARKDOWN_CACHE_MAX_ENTRIES {
            guard.clear();
        }
        guard.insert(key, rendered.clone());
    }
    rendered
}

/// LC-154: is a markdown link/image destination safe to emit as an `href` /
/// `src`? Allows relative URLs (fragment, query, path, or scheme-less) and the
/// `http`/`https`/`mailto` schemes; rejects `javascript:`, `data:`, `vbscript:`
/// and anything else that could execute on click.
fn link_scheme_is_safe(dest: &str) -> bool {
    let d = dest.trim();
    // Relative / fragment / query / protocol-relative-but-pathless are safe;
    // they cannot carry an executable scheme.
    if d.starts_with('/') || d.starts_with('#') || d.starts_with('?') {
        return true;
    }
    match d.split_once(':') {
        // No colon: scheme-less (relative) reference.
        None => true,
        // A colon that appears after a '/' is part of the path, not a scheme
        // (e.g. `foo/bar:baz`), so the reference is relative and safe.
        Some((scheme, _)) if scheme.contains('/') => true,
        Some((scheme, _)) => {
            matches!(
                scheme.trim().to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto"
            )
        }
    }
}

fn render_inner(body: &str, mentions: &[MentionRef], emojis: &[EmojiRef]) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(body, options);

    let mut in_code_block: Option<String> = None;
    let mut code_buf = String::new();
    let mut link_depth: usize = 0;

    let events = parser.filter_map(move |ev| match ev {
        // Drop raw HTML so user input can never inject markup.
        Event::Html(_) | Event::InlineHtml(_) => None,
        // Treat single newlines as hard breaks. Chat users expect what they
        // typed; CommonMark would otherwise collapse soft breaks into spaces.
        Event::SoftBreak => Some(Event::HardBreak),
        Event::Start(Tag::CodeBlock(kind)) => {
            in_code_block = Some(match kind {
                CodeBlockKind::Fenced(s) => s.to_string(),
                CodeBlockKind::Indented => String::new(),
            });
            code_buf.clear();
            None
        }
        Event::End(TagEnd::CodeBlock) => {
            let lang = in_code_block.take().unwrap_or_default();
            let html = highlight_code(&code_buf, &lang);
            Some(Event::Html(html.into()))
        }
        Event::Text(t) if in_code_block.is_some() => {
            code_buf.push_str(&t);
            None
        }
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            link_depth += 1;
            // LC-154: pulldown_cmark writes the destination into `href`
            // verbatim, so `[x](javascript:alert(1))` would render a
            // click-to-execute XSS anchor. Neutralize any destination whose
            // scheme is not on the allowlist by pointing it at `#`.
            let dest_url = if link_scheme_is_safe(&dest_url) {
                dest_url
            } else {
                "#".into()
            };
            Some(Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }))
        }
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            // Same allowlist for image sources (defense-in-depth; a
            // `javascript:` img src does not execute in modern browsers, but a
            // disallowed scheme has no business in an `<img src>`).
            let dest_url = if link_scheme_is_safe(&dest_url) {
                dest_url
            } else {
                "#".into()
            };
            Some(Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }))
        }
        ev @ Event::End(TagEnd::Link) => {
            link_depth = link_depth.saturating_sub(1);
            Some(ev)
        }
        Event::Code(t) => {
            // Inline code: literal text, no mention/emoji/link expansion.
            let html = format!(
                "<code class=\"lc-md-code\">{}</code>",
                html_escape(t.as_ref())
            );
            Some(Event::Html(html.into()))
        }
        Event::Text(t) => {
            // Text inside a link label keeps the link target intact - run
            // emoji/escape but skip linkify so we don't nest anchors.
            let html = if link_depth > 0 {
                let emoji_map: std::collections::HashMap<&str, &EmojiRef> =
                    emojis.iter().map(|e| (e.shortcode.as_str(), e)).collect();
                let mut buf = String::new();
                emojify_and_escape(t.as_ref(), &emoji_map, &mut buf);
                buf
            } else {
                render_body(t.as_ref(), mentions, emojis)
            };
            Some(Event::Html(html.into()))
        }
        other => Some(other),
    });

    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

/// Render a markdown string for a low-trust surface (currently the
/// login-page heading body, LC-96). Strips raw HTML and code blocks
/// entirely so the renderer never has to load syntect for a page
/// that mostly renders once per visitor. Mentions and custom emojis
/// are intentionally NOT processed - the login page has neither
/// context.
pub fn render_login_body(body: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(body, options);
    let mut in_code_block = false;
    let events = parser.filter_map(move |ev| match ev {
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::SoftBreak => Some(Event::HardBreak),
        Event::Start(Tag::CodeBlock(_)) => {
            in_code_block = true;
            None
        }
        Event::End(TagEnd::CodeBlock) => {
            in_code_block = false;
            None
        }
        _ if in_code_block => None,
        other => Some(other),
    });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

/// Eagerly load syntect's bundled syntaxes and themes. Both sets are
/// deserialized from large embedded byte arrays; the first synchronous
/// `get_or_init` call inside an async handler blocks the calling tokio
/// worker thread for several seconds and starves every other task
/// scheduled on the runtime until it completes. Calling this at startup
/// before the server binds means the cold-load cost is paid once,
/// inline, where it cannot interfere with request serving.
pub fn warm_syntect() {
    let _ = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines);
    let _ = THEME_SET.get_or_init(ThemeSet::load_defaults);
}

/// Syntax-highlight a fenced code block to a self-contained `<pre>` element
/// with inline styles. Unknown languages fall back to plain-text highlighting
/// (escaped body in a `<pre><code>`).
fn highlight_code(code: &str, lang: &str) -> String {
    let ss = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines);
    let ts = THEME_SET.get_or_init(ThemeSet::load_defaults);
    let theme = ts
        .themes
        .get("InspiredGitHub")
        .or_else(|| ts.themes.values().next())
        .expect("syntect ships at least one default theme");

    let syntax = if lang.is_empty() {
        ss.find_syntax_plain_text()
    } else {
        ss.find_syntax_by_token(lang)
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    };

    match syntect::html::highlighted_html_for_string(code, ss, syntax, theme) {
        Ok(html) => html,
        Err(_) => format!("<pre><code>{}</code></pre>", html_escape(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_bold_and_italic() {
        let out = render("**bold** and *italic*", &[], &[]);
        assert!(out.contains("<strong>bold</strong>"), "no bold: {out}");
        assert!(out.contains("<em>italic</em>"), "no italic: {out}");
    }

    #[test]
    fn renders_inline_code_literal() {
        let out = render("use `*foo*` not bold", &[], &[]);
        assert!(out.contains("<code"), "no inline code: {out}");
        assert!(out.contains("*foo*"), "inline code literal lost: {out}");
        assert!(!out.contains("<strong>foo</strong>"), "wrong: {out}");
    }

    #[test]
    fn renders_fenced_code_block_with_highlighting() {
        let body = "```rust\nfn main() {}\n```";
        let out = render(body, &[], &[]);
        assert!(out.contains("<pre"), "no pre tag: {out}");
        // syntect emits inline `style=` attributes for tokens.
        assert!(out.contains("style="), "no inline style: {out}");
        assert!(out.contains("main"), "code text lost: {out}");
    }

    #[test]
    fn unknown_language_falls_back_to_plain() {
        let body = "```nosuchlang\nhello\n```";
        let out = render(body, &[], &[]);
        assert!(out.contains("<pre"), "no pre tag: {out}");
        assert!(out.contains("hello"), "body lost: {out}");
    }

    #[test]
    fn mentions_render_inside_markdown_paragraph() {
        let mentions = vec![MentionRef {
            user_id: "alice-id".into(),
            username: "alice".into(),
        }];
        let out = render("hey **@alice** check", &mentions, &[]);
        assert!(
            out.contains(r#"<a href="/profile/alice-id""#),
            "no chip: {out}"
        );
        assert!(out.contains("<strong>"), "bold lost: {out}");
    }

    #[test]
    fn mentions_not_substituted_inside_code_block() {
        let mentions = vec![MentionRef {
            user_id: "alice-id".into(),
            username: "alice".into(),
        }];
        let body = "```\n@alice in code\n```";
        let out = render(body, &mentions, &[]);
        assert!(
            !out.contains(r#"<a href="/profile/alice-id""#),
            "chip leaked into code: {out}"
        );
        assert!(out.contains("@alice"), "literal lost: {out}");
    }

    #[test]
    fn mentions_not_substituted_inside_inline_code() {
        let mentions = vec![MentionRef {
            user_id: "alice-id".into(),
            username: "alice".into(),
        }];
        let out = render("ping `@alice` literally", &mentions, &[]);
        assert!(
            !out.contains(r#"<a href="/profile/alice-id""#),
            "chip leaked: {out}"
        );
        assert!(out.contains("@alice"), "literal lost: {out}");
    }

    #[test]
    fn raw_html_tags_are_dropped() {
        // Tags must not survive; the text between them falls through to
        // pulldown_cmark's plain-text events and is HTML-escaped on output,
        // so it renders as literal characters rather than executing.
        let out = render("hello <script>alert(1)</script> world", &[], &[]);
        assert!(!out.contains("<script"), "script open tag rendered: {out}");
        assert!(
            !out.contains("</script"),
            "script close tag rendered: {out}"
        );
        assert!(out.contains("hello"), "leading text lost: {out}");
        assert!(out.contains("world"), "trailing text lost: {out}");
    }

    #[test]
    fn embedded_tag_attributes_are_dropped() {
        // An <img onerror=...> would be dangerous if it survived. Ensure the
        // entire tag is stripped.
        let out = render("see <img src=x onerror=alert(1)> here", &[], &[]);
        assert!(!out.contains("<img"), "img tag rendered: {out}");
        assert!(!out.contains("onerror"), "attribute rendered: {out}");
    }

    #[test]
    fn soft_break_renders_as_line_break() {
        let out = render("line one\nline two", &[], &[]);
        assert!(out.contains("<br"), "no <br>: {out}");
    }

    #[test]
    fn javascript_link_scheme_is_neutralized() {
        let out = render("[click](javascript:alert(document.cookie))", &[], &[]);
        assert!(!out.contains("javascript:"), "js scheme survived: {out}");
        assert!(out.contains(r##"href="#""##), "not neutralized: {out}");
        assert!(out.contains("click"), "label lost: {out}");
    }

    #[test]
    fn data_and_vbscript_link_schemes_are_neutralized() {
        for bad in [
            "[x](data:text/plain;base64,AAAA)",
            "[x](vbscript:msgbox)",
            "[x](VBScript:msgbox)", // case-insensitive
        ] {
            let out = render(bad, &[], &[]);
            assert!(
                !out.to_lowercase().contains("data:") && !out.to_lowercase().contains("vbscript:"),
                "scheme survived for {bad}: {out}"
            );
        }
    }

    #[test]
    fn safe_link_schemes_are_preserved() {
        let out = render(
            "[a](https://example.com/p) [b](/rel/path) [c](mailto:x@y.z)",
            &[],
            &[],
        );
        assert!(
            out.contains(r#"href="https://example.com/p""#),
            "https lost: {out}"
        );
        assert!(out.contains(r#"href="/rel/path""#), "relative lost: {out}");
        assert!(out.contains(r#"href="mailto:x@y.z""#), "mailto lost: {out}");
    }

    #[test]
    fn link_scheme_predicate_matrix() {
        assert!(link_scheme_is_safe("https://x.com"));
        assert!(link_scheme_is_safe("http://x.com"));
        assert!(link_scheme_is_safe("mailto:a@b.c"));
        assert!(link_scheme_is_safe("/relative"));
        assert!(link_scheme_is_safe("#frag"));
        assert!(link_scheme_is_safe("example.com/path")); // scheme-less
        assert!(link_scheme_is_safe("path/to:thing")); // colon in path, not a scheme
        assert!(!link_scheme_is_safe("javascript:alert(1)"));
        assert!(!link_scheme_is_safe("  javascript:alert(1)")); // leading ws
        assert!(!link_scheme_is_safe("data:text/html,x"));
        assert!(!link_scheme_is_safe("vbscript:x"));
        assert!(!link_scheme_is_safe("file:///etc/passwd"));
    }

    #[test]
    fn url_in_link_label_is_not_double_linkified() {
        // Markdown link with a URL as its label - we should emit exactly one
        // <a> wrapping the label text, not a nested anchor from linkify.
        let out = render("[https://example.com](https://other.example)", &[], &[]);
        assert_eq!(out.matches("<a ").count(), 1, "anchor count wrong: {out}");
        assert!(out.contains(r#"href="https://other.example""#));
    }

    #[test]
    fn bare_url_in_paragraph_is_linkified() {
        let out = render("see https://example.com here", &[], &[]);
        assert!(
            out.contains(r#"<a href="https://example.com""#),
            "no link: {out}"
        );
    }

    #[test]
    fn list_renders() {
        let out = render("- one\n- two", &[], &[]);
        assert!(out.contains("<ul>"), "no <ul>: {out}");
        assert!(out.contains("<li>one"), "no li: {out}");
    }
}
