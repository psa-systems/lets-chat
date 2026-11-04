//! LC-538: the browser tab / window title is the product name "Let's Chat"
//! (title case, apostrophe), never the lowercase hyphenated system identifier
//! `lets-chat`. The hyphenated form is reserved for single-token/repo/system
//! references (crate name, `LETS_CHAT_*` env vars, `X-LetsChat-*` headers,
//! user-agent strings, urn/UID identifiers) that cannot carry an apostrophe -
//! it must not leak into user-visible title text.
//!
//! This is a structural grep-ban over `templates/`: every Askama `<title>` /
//! `{% block title %}` line must render "Let's Chat" and must not contain the
//! literal `lets-chat`. It mechanically prevents a new page from reintroducing
//! the lowercase form in the tab title (the exact drift LC-538 standardized).

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

/// A line that defines the tab/window title: the raw `<title>` element, the
/// Askama `block title` override that fills it, or the `data-base-title`
/// attribute that `layout.html`'s unread-count script reads to rebuild
/// `document.title` (so it is the live tab title just as much as `<title>`).
fn is_title_line(line: &str) -> bool {
    line.contains("<title>") || line.contains("block title") || line.contains("data-base-title")
}

#[test]
fn tab_title_uses_product_name_not_system_identifier() {
    let mut files = Vec::new();
    walk(Path::new("templates"), &mut files);
    assert!(
        !files.is_empty(),
        "no .html files found under templates/ - test must run from the server crate root"
    );

    let mut violations: Vec<String> = Vec::new();
    let mut title_lines_seen = 0usize;
    for file in &files {
        let body = std::fs::read_to_string(file).expect("read template");
        for (n, line) in body.lines().enumerate() {
            if !is_title_line(line) {
                continue;
            }
            title_lines_seen += 1;
            if line.contains("lets-chat") {
                violations.push(format!(
                    "{}:{}: tab/window title contains the lowercase system identifier `lets-chat`; use \"Let's Chat\"\n      | {}",
                    file.display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        title_lines_seen > 0,
        "found no <title>/block title lines under templates/ - the scan is not matching anything (bug in the test)"
    );

    if !violations.is_empty() {
        let body = violations.join("\n");
        panic!(
            "\n\nLC-538 tab-title naming violations.\n\
             \n\
             The browser tab / window title must read \"Let's Chat\" (title case,\n\
             apostrophe). The lowercase hyphenated `lets-chat` is the system/repo\n\
             identifier and must not appear in user-visible title text.\n\
             \n\
             Offending title lines:\n{body}\n"
        );
    }
}
