//! LC-59 server-side LaTeX math rendering.
//!
//! Detects inline `$...$` and display `$$...$$` math spans in message body
//! text and renders them to MathML via `pulldown-latex`. Non-math segments
//! flow through the existing chat pipeline (`views::room::render_body`:
//! mention chips, custom emoji, URL linkify) unchanged. Math wins inside
//! its own span: chip / emoji / URL detection never runs on math source,
//! the same way it never runs inside code spans.
//!
//! Entry point: [`render_math_in_text`]. Called from
//! `markdown::render_inner`'s `Event::Text` branch (outside fenced and
//! inline code), replacing the direct `render_body` call there.
//!
//! ## Safety
//!
//! `pulldown-latex` implements `\def`-family macros whose unbounded
//! expansion **stack-overflows the process** (verified in the LC-59 spike
//! with `\def\x{\x\x}\x`). Stack overflow is **uncatchable by
//! `catch_unwind`** in Rust; bounded-stack worker threads do not help
//! either (the runtime's stack-overflow handler is unconditional
//! `abort()`). Macro recursion therefore has to be prevented PRE-RENDER.
//!
//! [`blocklist_re`] rejects any span containing a control sequence that
//! can define, redefine, or alias another control sequence. It is a
//! denylist, validated against `pulldown-latex` 0.7.1's known macro
//! surface and the evasions exercised in the LC-59 spike (whitespace
//! tolerance, `\let` aliasing, `\csname`, TeX comments). Strong, not a
//! proof. The dependency is pinned to `=0.7.1` so a routine `cargo
//! update` cannot silently invalidate it; re-run the spike before
//! merging any version bump.
//!
//! Deep nesting (`\frac`, `\sqrt`, braces) is safe at any depth the
//! length cap permits: the parser is iterative, not call-stack recursive
//! on input structure (spike-confirmed to depth 6400 with no overflow).
//!
//! ## Failure-mode convergence
//!
//! Four triggers fall back to literal escaped text (the raw `$...$` or
//! `$$...$$` source as the user typed it):
//!
//! 1. Cap exceeded (span length, span count, or total math chars).
//! 2. Blocklist hit on the span source.
//! 3. `push_mathml` returned `Err` (uncommon: most parse failures come
//!    through as `Ok(())` with an `<merror>` element inside the output).
//! 4. Output contains the `<merror>` element. Detection is a substring
//!    scan on the rendered string; this is false-positive-safe because
//!    `<merror>` is renderer-emitted and never derived from user text
//!    content (user text becomes `<mi>` / `<mn>` / `<mo>`).
//!
//! Plus a defence-in-depth fifth: `catch_unwind` around the render call
//! catches non-stack-overflow panics from the crate.

use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;

use regex::Regex;

use crate::db::mentions::MentionRef;
use crate::models::custom_emoji::EmojiRef;
use crate::views::channel_complete::ChannelRef;
use crate::views::room::{html_escape, render_body};

/// Max source-character length of a single math span. Spans larger than
/// this fall back to literal text. Direction-to-adjust: widen, never
/// tighten - tightening breaks already-posted messages.
const MATH_MAX_SPAN_CHARS: usize = 1024;

/// Max number of math spans ATTEMPTED per message. Counts every scanned
/// math chunk regardless of whether it ended up rendered as MathML or
/// fell back to literal text. Bounding attempts (rather than successes)
/// keeps an unbounded number of blocklist-hit or malformed spans from
/// consuming arbitrary scan / blocklist / catch_unwind work; spans
/// past the cap are kept as literal text.
const MATH_MAX_SPANS_PER_MESSAGE: usize = 32;

/// Max total math source characters ATTEMPTED per message, summed across
/// every scanned math chunk regardless of render outcome. Same rationale
/// as `MATH_MAX_SPANS_PER_MESSAGE`: bound work, not output.
const MATH_MAX_TOTAL_CHARS: usize = 4096;

/// Control sequences whose presence in a span source rejects the span
/// pre-render. The macro-definers (`\def`, `\edef`, ..., `\providecommand`)
/// and the aliasing primitive (`\let`) are the load-bearing entries:
/// without them, a self-referential macro stack-overflows the process.
/// `\href` and `\url` are future-proof entries (currently unimplemented
/// in 0.7.1 - the spike confirmed they return `<merror>` - but if a
/// future version adds them, macro-driven link injection becomes the
/// XSS escape route the blocklist must close). `\csname` is future-
/// proof against dynamic control-sequence construction.
///
/// Word-boundary regex: `\def`, `\def `, `\def{`, `\def\x` all match,
/// `\definecolor` does NOT match. The boundary matters because the
/// LC-59 spike confirmed `\def \x{a}\x` (whitespace between command
/// and target) is a real evasion against a literal-substring blocklist.
fn blocklist_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\\(?:def|edef|xdef|gdef|let|futurelet|newcommand|renewcommand|providecommand|href|url|csname)\b",
        )
        .expect("valid blocklist regex")
    })
}

/// One scanned segment of the input text. Math chunks carry both the
/// `full` range (delimiters included, used for literal-fallback) and
/// the `content` range (delimiters stripped, fed to the renderer).
#[derive(Debug, PartialEq)]
enum Chunk {
    Text {
        range: Range<usize>,
    },
    Math {
        full: Range<usize>,
        content: Range<usize>,
        display: bool,
    },
}

/// Split `text` into alternating text and math chunks. Math span rules
/// (pandoc-style "tex_math_dollars" boundary discipline, adopted to keep
/// chat content like `$10 to $20` from being misread as math):
///
/// - Display `$$X$$` and inline `$X$`. Display is tried first at every
///   `$`; only on display-fail is inline tried at that position.
/// - `X` (the content) MUST be non-empty AND contain no `$` characters.
/// - The opening delimiter must be immediately followed by a
///   non-whitespace character.
/// - The closing delimiter must be immediately preceded by a
///   non-whitespace character.
/// - The closing delimiter must not be immediately followed by a digit
///   (so `$1$2` and `$$x$$5` are NOT math; users get the chat-text
///   reading instead of the surprising math reading).
/// - Unclosed `$` (or `$$`) at EOF: the opener is literal text; scanning
///   continues at the next position.
/// - Best-effort recovery: at every `$`, the scanner tries display then
///   inline; on both-fail it advances one byte and tries again. So
///   `$$a$b$$` (malformed display, content has `$`) recovers as literal
///   `$` + inline `$a$` + literal `b$$`. Defensible "find what you can"
///   policy; the alternative (bail on the whole malformed display block)
///   wastes a valid inner inline span.
/// - The scanner does not interpret backslash escapes (`\$`). By the
///   time text reaches us via `pulldown_cmark::Event::Text`, CommonMark
///   escape processing has already collapsed `\$` to `$`. Users who
///   want a literal `$` should wrap it in a code span (`` `$5` ``); that
///   is the existing chat convention for "show this verbatim".
fn scan(text: &str) -> Vec<Chunk> {
    let bytes = text.as_bytes();
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut last_text_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }

        if let Some((content, end)) = try_display_at(bytes, i) {
            if last_text_start < i {
                chunks.push(Chunk::Text {
                    range: last_text_start..i,
                });
            }
            chunks.push(Chunk::Math {
                full: i..end,
                content,
                display: true,
            });
            i = end;
            last_text_start = i;
            continue;
        }

        if let Some((content, end)) = try_inline_at(bytes, i) {
            if last_text_start < i {
                chunks.push(Chunk::Text {
                    range: last_text_start..i,
                });
            }
            chunks.push(Chunk::Math {
                full: i..end,
                content,
                display: false,
            });
            i = end;
            last_text_start = i;
            continue;
        }

        i += 1;
    }

    if last_text_start < bytes.len() {
        chunks.push(Chunk::Text {
            range: last_text_start..bytes.len(),
        });
    }
    chunks
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Try to match `$$X$$` at byte position `i`. Returns the content range
/// and the byte position immediately after the closing `$$`.
fn try_display_at(bytes: &[u8], i: usize) -> Option<(Range<usize>, usize)> {
    if i + 1 >= bytes.len() || bytes[i + 1] != b'$' {
        return None;
    }
    let content_start = i + 2;
    if content_start >= bytes.len() {
        return None;
    }
    // Open `$$` must be followed by non-whitespace (pandoc rule).
    if is_space(bytes[content_start]) {
        return None;
    }
    let mut j = content_start;
    while j < bytes.len() {
        if bytes[j] == b'$' {
            if j + 1 < bytes.len() && bytes[j + 1] == b'$' {
                if j == content_start {
                    return None; // empty content
                }
                // Close `$$` must be preceded by non-whitespace, and must
                // not be followed by a digit.
                if is_space(bytes[j - 1]) {
                    return None;
                }
                if j + 2 < bytes.len() && bytes[j + 2].is_ascii_digit() {
                    return None;
                }
                return Some((content_start..j, j + 2));
            }
            return None; // a single `$` inside what would be display content
        }
        j += 1;
    }
    None
}

/// Try to match `$X$` at byte position `i`. Caller has already ruled out
/// `$$` at `i` (display-first). Returns the content range and the byte
/// position immediately after the closing `$`.
fn try_inline_at(bytes: &[u8], i: usize) -> Option<(Range<usize>, usize)> {
    let content_start = i + 1;
    if content_start >= bytes.len() {
        return None;
    }
    // Open `$` must be followed by non-whitespace (pandoc rule).
    if is_space(bytes[content_start]) {
        return None;
    }
    let mut j = content_start;
    while j < bytes.len() {
        if bytes[j] == b'$' {
            if j == content_start {
                return None; // empty content
            }
            // Close `$` must be preceded by non-whitespace, and must not
            // be followed by a digit (keeps `$10 to $20` from matching).
            if is_space(bytes[j - 1]) {
                return None;
            }
            if j + 1 < bytes.len() && bytes[j + 1].is_ascii_digit() {
                return None;
            }
            return Some((content_start..j, j + 1));
        }
        j += 1;
    }
    None
}

/// Render a single math span to MathML or return `None` on any failure
/// mode (length-cap, blocklist hit, render error, `<merror>` in output,
/// or caught panic). Caller substitutes literal text on `None`.
///
/// `catch_unwind` safety: each call constructs its own fresh `Storage`
/// and `Parser`. No state is shared across calls. A caught panic
/// abandons exactly this span; the next call starts clean. (Stack
/// overflow is NOT caught here - the blocklist is the primary
/// mitigation for that class.)
fn try_render_math(source: &str, display: bool) -> Option<String> {
    if source.len() > MATH_MAX_SPAN_CHARS {
        return None;
    }
    if blocklist_re().is_match(source) {
        return None;
    }
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let storage = pulldown_latex::Storage::new();
        let parser = pulldown_latex::Parser::new(source, &storage);
        let mut out = String::new();
        let mut cfg = pulldown_latex::config::RenderConfig::default();
        if display {
            cfg.display_mode = pulldown_latex::config::DisplayMode::Block;
        }
        pulldown_latex::mathml::push_mathml(&mut out, parser, cfg)
            .ok()
            .map(|()| out)
    }));
    match result {
        Ok(Some(html)) if !html.contains("<merror") => Some(html),
        _ => None,
    }
}

/// Public entry. Replaces the bare `render_body(text, mentions, emojis)`
/// call inside `markdown::render_inner`'s `Event::Text` branch. Scans
/// `text` for math spans, renders each math span to MathML (with literal
/// fallback on any failure), and routes the surrounding non-math text
/// through `render_body` for mention / emoji / URL processing.
pub fn render_math_in_text(
    text: &str,
    mentions: &[MentionRef],
    emojis: &[EmojiRef],
    channels: &[ChannelRef],
) -> String {
    let chunks = scan(text);
    // Fast path: no math at all - call `render_body` once and skip the
    // chunk-iteration overhead entirely. Preserves the original behaviour
    // for the overwhelmingly common case of non-math messages.
    if !chunks.iter().any(|c| matches!(c, Chunk::Math { .. })) {
        return render_body(text, mentions, emojis, channels);
    }

    let mut out = String::with_capacity(text.len() + 64);
    let mut span_count: usize = 0;
    let mut total_math_chars: usize = 0;
    for chunk in chunks {
        match chunk {
            Chunk::Text { range } => {
                out.push_str(&render_body(&text[range], mentions, emojis, channels));
            }
            Chunk::Math {
                full,
                content,
                display,
            } => {
                let source = &text[content];
                let over_count = span_count >= MATH_MAX_SPANS_PER_MESSAGE;
                let over_total =
                    total_math_chars.saturating_add(source.len()) > MATH_MAX_TOTAL_CHARS;
                let rendered = if over_count || over_total {
                    None
                } else {
                    try_render_math(source, display)
                };
                span_count += 1;
                total_math_chars = total_math_chars.saturating_add(source.len());
                match rendered {
                    Some(html) => out.push_str(&html),
                    None => out.push_str(&html_escape(&text[full])),
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- scan: simple cases ----------

    #[test]
    fn scan_empty_input() {
        assert_eq!(scan(""), vec![]);
    }

    #[test]
    fn scan_text_only_no_math() {
        let chunks = scan("hello world");
        assert_eq!(chunks, vec![Chunk::Text { range: 0..11 }]);
    }

    #[test]
    fn scan_lone_dollar_is_literal() {
        let chunks = scan("$");
        assert_eq!(chunks, vec![Chunk::Text { range: 0..1 }]);
    }

    #[test]
    fn scan_simple_inline_math() {
        let chunks = scan("$x^2$");
        assert_eq!(
            chunks,
            vec![Chunk::Math {
                full: 0..5,
                content: 1..4,
                display: false
            }]
        );
    }

    #[test]
    fn scan_simple_display_math() {
        let chunks = scan("$$x^2$$");
        assert_eq!(
            chunks,
            vec![Chunk::Math {
                full: 0..7,
                content: 2..5,
                display: true
            }]
        );
    }

    #[test]
    fn scan_text_then_math_then_text() {
        let chunks = scan("pre $x$ post");
        assert_eq!(
            chunks,
            vec![
                Chunk::Text { range: 0..4 },
                Chunk::Math {
                    full: 4..7,
                    content: 5..6,
                    display: false
                },
                Chunk::Text { range: 7..12 },
            ]
        );
    }

    #[test]
    fn scan_unclosed_dollar_is_literal() {
        let chunks = scan("$x^2 has no closer");
        assert_eq!(chunks, vec![Chunk::Text { range: 0..18 }]);
    }

    // ---------- scan: adjacent-delimiter battery (LC-59 explicit pinning) ----------

    #[test]
    fn scan_adjacent_two_inline_spans_no_gap() {
        // $a$$b$ -> inline(a) + inline(b), NOT a mid-display open
        let chunks = scan("$a$$b$");
        assert_eq!(
            chunks,
            vec![
                Chunk::Math {
                    full: 0..3,
                    content: 1..2,
                    display: false
                },
                Chunk::Math {
                    full: 3..6,
                    content: 4..5,
                    display: false
                },
            ]
        );
    }

    #[test]
    fn scan_bare_double_dollar_is_literal() {
        // $$ alone -> literal $$, not an empty display
        let chunks = scan("$$");
        assert_eq!(chunks, vec![Chunk::Text { range: 0..2 }]);
    }

    #[test]
    fn scan_triple_dollar_is_literal() {
        // $$$ -> literal $$$. The first $$ has no closer; the trailing $
        // has no closer either.
        let chunks = scan("$$$");
        assert_eq!(chunks, vec![Chunk::Text { range: 0..3 }]);
    }

    #[test]
    fn scan_quadruple_dollar_is_literal() {
        // $$$$ -> literal $$$$. Display would need non-empty content;
        // every offset produces either empty content or no closer.
        let chunks = scan("$$$$");
        assert_eq!(chunks, vec![Chunk::Text { range: 0..4 }]);
    }

    #[test]
    fn scan_inline_with_space_separator() {
        let chunks = scan("$a$ $b$");
        assert_eq!(
            chunks,
            vec![
                Chunk::Math {
                    full: 0..3,
                    content: 1..2,
                    display: false
                },
                Chunk::Text { range: 3..4 },
                Chunk::Math {
                    full: 4..7,
                    content: 5..6,
                    display: false
                },
            ]
        );
    }

    #[test]
    fn scan_display_then_inline() {
        let chunks = scan("$$x$$ $y$");
        assert_eq!(
            chunks,
            vec![
                Chunk::Math {
                    full: 0..5,
                    content: 2..3,
                    display: true
                },
                Chunk::Text { range: 5..6 },
                Chunk::Math {
                    full: 6..9,
                    content: 7..8,
                    display: false
                },
            ]
        );
    }

    #[test]
    fn scan_dollar_inside_display_recovers_inline_match() {
        // `$$a$b$$` is a malformed display (content can't contain `$`).
        // The scanner advances one byte and finds the inner `$a$` as an
        // inline span. Best-effort recovery rather than bailing on the
        // whole construct.
        let chunks = scan("$$a$b$$");
        assert_eq!(
            chunks,
            vec![
                Chunk::Text { range: 0..1 },
                Chunk::Math {
                    full: 1..4,
                    content: 2..3,
                    display: false
                },
                Chunk::Text { range: 4..7 },
            ]
        );
    }

    // ---------- scan: pandoc-rule boundaries ----------

    #[test]
    fn scan_currency_pair_is_not_math() {
        // `$10 to $20`: closing `$` at pos 7 preceded by ' ' (whitespace).
        // Pandoc rule rejects; the whole string is literal.
        let chunks = scan("$10 to $20");
        assert!(
            chunks.iter().all(|c| matches!(c, Chunk::Text { .. })),
            "currency matched as math: {chunks:?}",
        );
    }

    #[test]
    fn scan_close_followed_by_digit_is_not_math() {
        // `$1$2`: close `$` at pos 2 is followed by digit '2'. Rejected.
        let chunks = scan("$1$2");
        assert!(
            chunks.iter().all(|c| matches!(c, Chunk::Text { .. })),
            "close-followed-by-digit matched: {chunks:?}",
        );
    }

    #[test]
    fn scan_open_followed_by_space_is_not_math() {
        // `$ x$`: open `$` followed by space. Rejected.
        let chunks = scan("$ x$");
        assert!(
            chunks.iter().all(|c| matches!(c, Chunk::Text { .. })),
            "space-after-open matched: {chunks:?}",
        );
    }

    #[test]
    fn scan_close_preceded_by_space_is_not_math() {
        // `$x $`: close `$` preceded by space. Rejected.
        let chunks = scan("$x $");
        assert!(
            chunks.iter().all(|c| matches!(c, Chunk::Text { .. })),
            "space-before-close matched: {chunks:?}",
        );
    }

    #[test]
    fn scan_display_close_followed_by_digit_rejects_display_then_recovers_inline() {
        // `$$x$$5`: the display rule rejects the `$$x$$` match because
        // `5` follows the close. The inline-recovery step still finds
        // `$x$` at offset 1..4. Documents both rules at once: the
        // display rule fires, AND the recovery still surfaces the inner
        // inline match. If a future change wants display-rule-failures
        // to suppress the inner recovery, this test will catch it.
        let chunks = scan("$$x$$5");
        assert_eq!(
            chunks,
            vec![
                Chunk::Text { range: 0..1 },
                Chunk::Math {
                    full: 1..4,
                    content: 2..3,
                    display: false
                },
                Chunk::Text { range: 4..6 },
            ]
        );
    }

    #[test]
    fn scan_display_open_followed_by_space_is_not_math() {
        // `$$ x $$`: open `$$` followed by space. Display fails cleanly,
        // and no inline recovery picks up anything: bytes[2]=' ' fails
        // open-followed-by-non-space at pos 1, and the closing `$$` has
        // a space before it too. All literal.
        let chunks = scan("$$ x $$");
        assert!(
            chunks.iter().all(|c| matches!(c, Chunk::Text { .. })),
            "display-open-space matched: {chunks:?}",
        );
    }

    #[test]
    fn scan_inline_with_punctuation_after_close_matches() {
        // Close `$` followed by ':' (or comma, period, space) is fine.
        let chunks = scan("$x$: rest");
        assert_eq!(
            chunks[0],
            Chunk::Math {
                full: 0..3,
                content: 1..2,
                display: false
            }
        );
    }

    #[test]
    fn scan_math_then_mention_then_math() {
        // The math chunks span the right offsets, leaving the mention
        // for `render_body` to handle in the surrounding text.
        let chunks = scan("$x$ hi @bob then $y$");
        assert_eq!(
            chunks,
            vec![
                Chunk::Math {
                    full: 0..3,
                    content: 1..2,
                    display: false
                },
                Chunk::Text { range: 3..17 },
                Chunk::Math {
                    full: 17..20,
                    content: 18..19,
                    display: false
                },
            ]
        );
    }

    // ---------- try_render_math: render boundary ----------

    #[test]
    fn render_simple_inline_produces_math_element() {
        let html = try_render_math("x^2", false).expect("should render");
        assert!(html.contains("<math"), "no <math>: {html}");
        assert!(html.contains("<msup>"), "no <msup>: {html}");
        assert!(!html.contains("<merror"), "unexpected merror: {html}");
    }

    #[test]
    fn render_display_emits_block_attribute() {
        let html = try_render_math(r"\int_0^1 f(x)\,dx", true).expect("should render");
        assert!(
            html.contains(r#"display="block""#) || html.contains("display='block'"),
            "no block attr: {html}",
        );
    }

    #[test]
    fn render_returns_none_when_output_carries_merror() {
        // Malformed input -> pulldown-latex returns Ok with `<merror>`
        // embedded. The wrapper must surface that as None so the caller
        // falls back to literal text instead of showing a red-bordered
        // error chunk in chat.
        assert_eq!(try_render_math(r"\frac{", false), None);
    }

    #[test]
    fn render_returns_none_for_oversize_span() {
        let big = "x".repeat(MATH_MAX_SPAN_CHARS + 1);
        assert_eq!(try_render_math(&big, false), None);
    }

    // ---------- try_render_math: blocklist ----------

    #[test]
    fn blocklist_rejects_def() {
        assert_eq!(try_render_math(r"\def\x{a}\x", false), None);
    }

    #[test]
    fn blocklist_rejects_def_with_whitespace() {
        // LC-59 spike evasion: `\def \x{a}\x` works as a real macro
        // definition under a literal-substring blocklist. Word boundary
        // closes that hole.
        assert_eq!(try_render_math("\\def \\x{a}\\x", false), None);
    }

    #[test]
    fn blocklist_rejects_let_alias() {
        // LC-59 spike evasion: `\let\foo=\def \foo\x{a}` aliases `\def`
        // through `\let`. Blocking `\let` itself closes the chain.
        assert_eq!(try_render_math(r"\let\foo=\def \foo\x{a}\x", false), None);
    }

    #[test]
    fn blocklist_rejects_all_macro_definers() {
        for cs in [
            r"\def\x{a}\x",
            r"\edef\x{a}\x",
            r"\xdef\x{a}\x",
            r"\gdef\x{a}\x",
            r"\let\x=\def",
            r"\futurelet\x\y a",
            r"\newcommand{\x}{a}\x",
            r"\renewcommand{\x}{a}\x",
            r"\providecommand{\x}{a}\x",
            r"\href{x}{y}",
            r"\url{http://x}",
            r"\csname x\endcsname",
        ] {
            assert_eq!(try_render_math(cs, false), None, "blocklist missed: {cs}");
        }
    }

    #[test]
    fn blocklist_does_not_match_unrelated_definecolor() {
        // `\definecolor` shares a prefix with `\def`. The word-boundary
        // regex MUST NOT block it (even though pulldown-latex doesn't
        // implement it - the test guards against a future addition that
        // would silently fail under a substring blocklist).
        let re = blocklist_re();
        assert!(!re.is_match(r"\definecolor{r}{rgb}{1,0,0}"));
    }

    // ---------- render_math_in_text: integration ----------

    #[test]
    fn integration_pure_text_takes_fast_path() {
        // No `$` in the input - render_body output verbatim, no <math> tag.
        let out = render_math_in_text("hello world", &[], &[], &[]);
        assert!(!out.contains("<math"), "spurious math element: {out}");
        assert!(out.contains("hello world"), "text lost: {out}");
    }

    #[test]
    fn integration_inline_math_typesets() {
        let out = render_math_in_text("$x^2$", &[], &[], &[]);
        assert!(out.contains("<math"), "no math: {out}");
        assert!(out.contains("<msup>"), "no msup: {out}");
    }

    #[test]
    fn integration_display_math_typesets_as_block() {
        let out = render_math_in_text("$$x^2$$", &[], &[], &[]);
        assert!(out.contains(r#"display="block""#), "not block: {out}",);
    }

    #[test]
    fn integration_math_plus_mention_both_render() {
        let mentions = vec![MentionRef {
            user_id: "bob-id".into(),
            username: "bob".into(),
        }];
        let out = render_math_in_text("see $x^2$ then @bob", &mentions, &[], &[]);
        assert!(out.contains("<math"), "no math: {out}");
        assert!(
            out.contains(r#"href="/profile/bob-id""#),
            "no mention chip: {out}",
        );
    }

    #[test]
    fn integration_at_user_inside_math_is_literal_not_a_chip() {
        // $@user$ - math wins inside the span. The `@user` is LaTeX
        // content, not a chat mention. No chip in the output.
        let mentions = vec![MentionRef {
            user_id: "bob-id".into(),
            username: "bob".into(),
        }];
        let out = render_math_in_text("$@bob$", &mentions, &[], &[]);
        // Positive 1: the math pipeline took ownership of the span -
        // either it rendered as MathML, or it fell back to literal
        // `$@bob$`. The failing case the negative-only test would have
        // missed is "math source dropped to nothing"; the positive
        // assertion catches that.
        assert!(
            out.contains("<math") || out.contains("$@bob$"),
            "math source disappeared from output: {out}",
        );
        // Positive 2: the `@bob` token survives in some form inside the
        // math (or as literal fallback). pulldown-latex splits each
        // identifier letter into its own `<mi>` element, so "bob"
        // doesn't appear contiguously - check for an `<mi>b</mi>`
        // (math path) or the literal `$@bob$` (fallback path).
        assert!(
            out.contains("<mi>b</mi>") || out.contains("$@bob$"),
            "@bob token lost from output: {out}",
        );
        // Negative: no mention chip rendered for the @bob inside the
        // math span. The render_body pipeline never saw `$@bob$`.
        assert!(
            !out.contains(r#"href="/profile/bob-id""#),
            "mention chip leaked into math: {out}",
        );
    }

    #[test]
    fn integration_blocklist_hit_falls_back_to_literal_with_delimiters() {
        let out = render_math_in_text(r"$\def\x{a}\x$", &[], &[], &[]);
        assert!(!out.contains("<math"), "math leaked: {out}");
        // The fallback includes the `$` delimiters (escaped) so the
        // user sees what they typed.
        assert!(out.contains("$"), "delimiters lost in fallback: {out}");
    }

    #[test]
    fn integration_oversize_span_falls_back_to_literal() {
        let big = format!("${}$", "x".repeat(MATH_MAX_SPAN_CHARS + 1));
        let out = render_math_in_text(&big, &[], &[], &[]);
        assert!(!out.contains("<math"), "math rendered past cap: {out}");
    }

    #[test]
    fn integration_span_count_cap_renders_first_n_then_literal() {
        // Build N+1 inline spans; first N typeset, the (N+1)th and onward
        // render as literal.
        let mut body = String::new();
        for _ in 0..MATH_MAX_SPANS_PER_MESSAGE + 2 {
            body.push_str("$x$ ");
        }
        let out = render_math_in_text(&body, &[], &[], &[]);
        let math_count = out.matches("<math").count();
        assert_eq!(
            math_count, MATH_MAX_SPANS_PER_MESSAGE,
            "cap not enforced: got {math_count} <math>",
        );
    }

    #[test]
    fn integration_malformed_math_falls_back_to_literal() {
        // `$\frac{$` is malformed; pulldown-latex emits `<merror>`; the
        // wrapper falls back to literal text containing the source.
        let out = render_math_in_text(r"$\frac{$", &[], &[], &[]);
        assert!(!out.contains("<math"), "merror leaked: {out}");
    }

    #[test]
    fn integration_unclosed_math_is_plain_text() {
        // No closer for `$x^2`: the scanner emits no math chunk; the
        // text flows through render_body unchanged.
        let out = render_math_in_text("$x^2 no close", &[], &[], &[]);
        assert!(!out.contains("<math"), "phantom math: {out}");
        assert!(out.contains("$x^2"), "text lost: {out}");
    }
}
