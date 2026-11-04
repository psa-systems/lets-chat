//! LC-742: every page-level top bar is the shared `.lc-header` skeleton.
//!
//! Sixteen page templates hand-rolled the same `border-b border-border px-4
//! py-2 flex items-center justify-between gap-2` string, giving the app four
//! different top-bar heights (36px hand-rolled, 56px `.lc-header`, and two
//! one-offs) that visibly jumped as the user navigated. These guards pin the
//! conversion: the hand-rolled geometry cannot come back, the converted bars
//! keep the class, and the admin bar keeps the same height floor and padding.
//!
//! The right-hand panel headers (`px-3 py-2`) and the compose panels
//! (`px-2 py-1.5`) are a separate, internally consistent group and are
//! deliberately out of scope, so the scan below keys on the page-bar gutter.

use std::path::{Path, PathBuf};

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Every `.html` under `root`, as (path, contents).
fn templates() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![manifest("templates")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {dir:?}: {e}")) {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("html") {
                let text =
                    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
                out.push((p, text));
            }
        }
    }
    assert!(!out.is_empty(), "no templates found");
    out
}

/// Drop Askama comments so prose about markup is not mistaken for markup.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{#") {
        out.push_str(&rest[..start]);
        match rest[start..].find("#}") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The text of every `<header ...>` start tag, from `<` to the matching `>`.
/// Attribute values can contain `>` inside an Askama expression, so this
/// tracks quotes rather than scanning for the next `>`.
fn header_tags(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Byte-wise: `i` walks past multi-byte characters, so `text[i..]` is
        // not always a char boundary and would panic.
        if !bytes[i..].starts_with(b"<header") {
            i += 1;
            continue;
        }
        // `<headerish` is not a `<header>`.
        if !matches!(bytes.get(i + 7), Some(b' ' | b'\t' | b'\n' | b'\r' | b'>')) {
            i += 1;
            continue;
        }
        let mut j = i + 7;
        let mut quote: Option<u8> = None;
        while j < bytes.len() {
            let c = bytes[j];
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if c == b'"' || c == b'\'' => quote = Some(c),
                None if c == b'>' => break,
                None => {}
            }
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        tags.push(text[i..j].to_string());
        i = j + 1;
    }
    tags
}

/// The page templates that own a top bar. Each entry's FIRST `<header>` is
/// that bar; later headers in the same file (the three inner section headers
/// in `transcripts/show.html`) are panel headers and are not checked.
const PAGE_BARS: [&str; 18] = [
    "activity/page.html",
    "enclave/branding.html",
    "enclave/page.html",
    "enclave/settings.html",
    "home/welcome.html",
    "inbox/page.html",
    "kudos/page.html",
    "reminders/page.html",
    "room/highlights.html",
    "room/info.html",
    "room/manage.html",
    "room/pins.html",
    "saved/page.html",
    "scheduled.html",
    "settings/blocked.html",
    "settings/page.html",
    "stats/page.html",
    "transcripts/index.html",
];

/// No page bar is hand-rolled again. `.lc-header` owns the border and the
/// 20px gutter that lines the bar's content up with the `px-5` timeline, so a
/// `<header>` that writes `border-b` at the page gutter is re-creating the
/// skeleton by hand and will drift from it.
#[test]
fn no_header_hand_rolls_the_page_top_bar() {
    let mut hand_rolled = Vec::new();
    for (path, text) in templates() {
        for tag in header_tags(&strip_comments(&text)) {
            let flat = tag.replace('\n', " ");
            let page_gutter = flat.contains(" px-4") || flat.contains(" px-5");
            if flat.contains("border-b") && page_gutter && !flat.contains("lc-header") {
                hand_rolled.push(format!("{}: {}", path.display(), flat));
            }
        }
    }
    assert!(
        hand_rolled.is_empty(),
        "page top bars that re-create the .lc-header skeleton by hand; use \
         `class=\"lc-header\"` and give bar content its own wrapper if it needs \
         a different alignment (LC-742):\n  {}",
        hand_rolled.join("\n  ")
    );
}

/// Every converted page bar still carries the class. Reverting one to a
/// utility string is what produced the four different bar heights.
#[test]
fn every_page_top_bar_uses_lc_header() {
    let mut missing = Vec::new();
    for rel in PAGE_BARS {
        let path = manifest("templates").join(rel);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        match header_tags(&strip_comments(&text)).first() {
            Some(tag) if tag.contains("lc-header") => {}
            Some(tag) => missing.push(format!("{rel}: {}", tag.replace('\n', " "))),
            None => missing.push(format!("{rel}: no <header> at all")),
        }
    }
    assert!(
        missing.is_empty(),
        "page top bars not on the shared .lc-header skeleton (LC-742):\n  {}",
        missing.join("\n  ")
    );
}

/// The admin console's bar sits in the same pane position as the app's, so it
/// keeps the same height floor and padding. Only the two declarations that
/// decide the bar's height are pinned; colors and layout are free to differ.
#[test]
fn admin_header_matches_lc_header_geometry() {
    let css = std::fs::read_to_string(manifest("assets/main.css")).expect("read main.css");
    let block = |selector: &str| {
        let start = css
            .find(&format!("\n{selector} {{"))
            .unwrap_or_else(|| panic!("{selector} not found in main.css"));
        let end = css[start..].find('}').expect("unterminated rule") + start;
        css[start..end].to_string()
    };
    for selector in [".lc-header", ".lc-admin-header"] {
        let rule = block(selector);
        for decl in ["min-height: 3.5rem;", "padding: 0.875rem 1.25rem;"] {
            assert!(
                rule.contains(decl),
                "{selector} must declare `{decl}` so the top bar does not \
                 resize between the app and the admin console (LC-742); got:\n{rule}"
            );
        }
    }
}
