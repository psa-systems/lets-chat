//! LC-734: a `<form>` wrapping a `[data-lc-search]` combobox must never fall
//! back to the browser's default submit. These forms have no `action`, so a
//! native submit GETs the current URL with `?q=<typed text>`, discarding the
//! room view and leaking the query into the address bar, history and `Referer`.
//!
//! `search.js` only calls `preventDefault()` on Enter when an option is
//! highlighted (the `keyup[key=='Enter']` htmx trigger needs the plain case to
//! propagate), so the guard has to live on the form: either a real `method`
//! (the form genuinely posts somewhere) or `onsubmit="return false"`.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir templates") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("html") {
            out.push(path);
        }
    }
}

/// True when `body` carries the bare opt-in marker `data-lc-search`, excluding
/// its relatives (`data-lc-search-results`, `-tab`, `-region`).
fn has_search_input(body: &str) -> bool {
    body.match_indices("data-lc-search").any(|(i, m)| {
        !matches!(
            body[i + m.len()..].chars().next(),
            Some('-') | Some('=') | Some('_')
        )
    })
}

fn line_of(body: &str, offset: usize) -> usize {
    body[..offset].matches('\n').count() + 1
}

#[test]
fn every_search_form_guards_native_submit() {
    let mut files = Vec::new();
    walk(Path::new("templates"), &mut files);
    assert!(
        !files.is_empty(),
        "no .html files found under templates/ - test must run from the server crate root"
    );

    let mut violations: Vec<String> = Vec::new();
    let mut guarded_forms_seen = 0usize;
    for file in &files {
        let body = std::fs::read_to_string(file).expect("read template");
        // Forms cannot nest in HTML, so pairing each `<form` with the next
        // `</form>` is exact.
        let mut cursor = 0usize;
        while let Some(rel) = body[cursor..].find("<form") {
            let open = cursor + rel;
            let tag_end = match body[open..].find('>') {
                Some(o) => open + o,
                None => break,
            };
            let close = match body[tag_end..].find("</form>") {
                Some(o) => tag_end + o,
                None => break,
            };
            let open_tag = &body[open..=tag_end];
            let inner = &body[tag_end..close];
            cursor = close + "</form>".len();

            if !has_search_input(inner) {
                continue;
            }
            if open_tag.contains("method=") || open_tag.contains("onsubmit=") {
                guarded_forms_seen += 1;
                continue;
            }
            violations.push(format!(
                "{}:{}: <form> wrapping a [data-lc-search] input has neither `method` nor an `onsubmit` guard\n      | {}",
                file.display(),
                line_of(&body, open),
                open_tag.trim()
            ));
        }
    }

    assert!(
        guarded_forms_seen > 0,
        "found no guarded <form> around a [data-lc-search] input - the scan is not matching anything (bug in the test)"
    );

    if !violations.is_empty() {
        let body = violations.join("\n");
        panic!(
            "\n\nLC-734 unguarded search form(s).\n\
             \n\
             A form around a [data-lc-search] combobox submits natively on Enter\n\
             when no result is highlighted, navigating to `<current-url>?q=...`.\n\
             Add `onsubmit=\"return false\"` (see partials/room_header.html).\n\
             \n\
             Offending forms:\n{body}\n"
        );
    }
}

/// The `autocomplete` attribute is only meaningful on `<form>` and on form
/// controls. LC-734 removed two copies that sat on a wrapper `<div>` in
/// `enclave/settings.html`, where the browser ignores them.
#[test]
fn autocomplete_is_never_set_on_a_plain_div() {
    let mut files = Vec::new();
    walk(Path::new("templates"), &mut files);

    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        let body = std::fs::read_to_string(file).expect("read template");
        for (n, line) in body.lines().enumerate() {
            if line.contains("<div") && line.contains("autocomplete=") {
                violations.push(format!(
                    "{}:{}: `autocomplete` on a <div> does nothing; put it on the input or the form\n      | {}",
                    file.display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }

    if !violations.is_empty() {
        let body = violations.join("\n");
        panic!("\n\nLC-734 dead `autocomplete` attribute(s):\n{body}\n");
    }
}
