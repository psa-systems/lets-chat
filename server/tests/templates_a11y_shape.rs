//! LC-750: template shape guards for the audit roll-up.
//!
//! Each test pins one thing the roll-up removed or established, so it cannot
//! quietly come back. Most read the templates and assets off disk the same way
//! `i18n_catalog.rs` reads the catalogs; the last one renders a page. Nothing
//! here needs a running server.

use askama::Template;
use std::path::{Path, PathBuf};

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Every file under `root` with the given extension, as (path, contents).
fn files(root: &Path, ext: &str) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {dir:?}: {e}")) {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                let text =
                    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
                out.push((p, text));
            }
        }
    }
    assert!(!out.is_empty(), "no .{ext} files under {root:?}");
    out
}

/// Drop Askama comments (`{# ... #}`) so prose about markup is not mistaken for
/// markup. Every scan below is about what actually renders.
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

/// The text of every HTML start tag in `text` (the span between `<` and the
/// matching `>`), skipping closing tags. Attribute values may contain `>`
/// inside Askama expressions, which is why this tracks quotes.
fn start_tags(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Closing tag / comment / doctype: not a start tag.
        if matches!(bytes.get(i + 1), Some(b'/') | Some(b'!') | None) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
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

/// True if `attr` appears in `tag` as its own attribute. Matches inside a
/// quoted value do not count (the composer's `oninput` handler contains a
/// comment with the word "required" in it), and neither does a match that is
/// part of a longer name (`ack-required` must not match `required`).
fn has_attr(tag: &str, attr: &str) -> bool {
    let bytes = tag.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) if c == q => {
                quote = None;
                i += 1;
                continue;
            }
            Some(_) => {
                i += 1;
                continue;
            }
            None if c == b'"' || c == b'\'' => {
                quote = Some(c);
                i += 1;
                continue;
            }
            None => {}
        }
        if tag[i..].starts_with(attr)
            && i > 0
            && matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r')
            && bytes
                .get(i + attr.len())
                .is_none_or(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'=' | b'>' | b'/'))
        {
            return true;
        }
        i += 1;
    }
    false
}

/// F28: every `<nav>` landmark names itself. With several unnamed ones on a
/// page, a screen reader's landmark list is a row of identical "navigation"
/// entries and the user cannot tell the room list from the enclave rail.
#[test]
fn every_nav_landmark_has_an_accessible_name() {
    let mut unnamed = Vec::new();
    for (path, text) in files(&manifest("templates"), "html") {
        for tag in start_tags(&strip_comments(&text)) {
            let name = tag[1..].split([' ', '\t', '\n', '\r', '>']).next().unwrap();
            if name == "nav" && !has_attr(&tag, "aria-label") && !has_attr(&tag, "aria-labelledby")
            {
                unnamed.push(format!("{}: {}", path.display(), tag.replace('\n', " ")));
            }
        }
    }
    assert!(
        unnamed.is_empty(),
        "<nav> landmarks with no accessible name (add aria-label with an i18n key, \
         or aria-labelledby pointing at a visible heading):\n  {}",
        unnamed.join("\n  ")
    );
}

/// F25: `required` and `aria-required="true"` travel together. The pair is what
/// the required marker in `partials/required_mark.html` documents visually, so
/// a control that has one without the other is half-converted.
#[test]
fn every_required_control_is_marked_aria_required() {
    let mut bare = Vec::new();
    for (path, text) in files(&manifest("templates"), "html") {
        for tag in start_tags(&strip_comments(&text)) {
            if has_attr(&tag, "required") && !has_attr(&tag, "aria-required") {
                bare.push(format!("{}: {}", path.display(), tag.replace('\n', " ")));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "controls with `required` but no `aria-required=\"true\"` (see the \
         Required fields section of docs/ui-conventions.md):\n  {}",
        bare.join("\n  ")
    );
}

/// F29: the sidebar runs on its own color ramp (`--sidebar-surface` is dark navy
/// even in light mode), so a fixed numbered shade there is the one color that
/// cannot move with the palette. The star toggle was the last one; it now uses
/// the `--star` token.
#[test]
fn sidebar_partials_use_color_tokens_not_raw_shades() {
    const HUES: [&str; 22] = [
        "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime",
        "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia",
        "pink", "rose",
    ];
    let mut raw = Vec::new();
    for (path, text) in files(&manifest("templates/partials"), "html") {
        for (n, line) in strip_comments(&text).lines().enumerate() {
            // `text-amber-500`, `hover:text-amber-600`, `bg-slate-700`, ...: a
            // hue segment followed immediately by a shade number.
            let hit = HUES.iter().any(|hue| {
                let needle = format!("-{hue}-");
                line.match_indices(&needle).any(|(i, _)| {
                    line[i + needle.len()..].starts_with(|c: char| c.is_ascii_digit())
                })
            });
            if hit {
                raw.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        raw.is_empty(),
        "raw numbered palette utilities in sidebar/shared partials; use the \
         semantic tokens from tailwind.config.js:\n  {}",
        raw.join("\n  ")
    );
}

/// F24: the htmx extension behind `hx-target-error` shipped on every page (a
/// script tag, `hx-ext` on `<body>`, a service-worker precache entry) with zero
/// call sites anywhere, so a failed save always showed the generic toast rather
/// than the server's reason. Either it is used or it is not loaded; loading it
/// unused is the state this removed.
///
/// The extension's name is assembled at run time on purpose, so that grepping
/// the tree for it keeps returning nothing at all, this guard included.
#[test]
fn error_target_extension_is_absent_or_actually_used() {
    let ext_name = format!("{}-targets", "response");
    let mut loaded = Vec::new();
    let mut used = false;
    for root in ["templates", "assets"] {
        let dir = manifest(root);
        let exts = if root == "templates" { "html" } else { "js" };
        for (path, text) in files(&dir, exts) {
            if text.contains("hx-target-error") {
                used = true;
            }
            if text.contains(&ext_name) {
                loaded.push(path.display().to_string());
            }
        }
    }
    assert!(
        used || loaded.is_empty(),
        "the htmx {ext_name} extension is loaded but nothing uses \
         hx-target-error; drop it or adopt it (LC-750 F24):\n  {}",
        loaded.join("\n  ")
    );
}

/// F25: `.input-error` was defined in `tailwind.css` with zero call sites, so a
/// rejected field looked identical to an accepted one and the whole validation
/// experience was the browser's native tooltip. Render the login-approval page
/// both ways: the class marks the field only when the server rejected the code,
/// and the reason still travels as text in the `role="alert"` banner.
#[test]
fn rejected_field_renders_input_error_and_a_text_reason() {
    let page = |error| lets_chat::views::login_approval::LoginApprovePage {
        asset_version: "test",
        app_version: "0.0.0",
        git_hash: "deadbeef",
        build_date: "1970-01-01",
        token: "tok",
        error,
    };

    let ok = page(None).render().expect("render without error");
    assert!(
        !ok.contains("input-error"),
        "an untouched field must not render as invalid"
    );

    let rejected = page(Some("That code is not right."))
        .render()
        .expect("render with error");
    assert!(
        rejected.contains("input-error"),
        "a server-rejected field must carry .input-error"
    );
    assert!(
        rejected.contains(r#"role="alert""#) && rejected.contains("That code is not right."),
        "the reason must stay readable as text, not just a red edge"
    );
}
