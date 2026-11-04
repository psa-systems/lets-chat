//! LC-1051: `<noscript>` may appear only in `templates/base.html` (`<head>`).
//!
//! htmx parses a boosted response with `DOMParser`, which runs with scripting
//! disabled, so the contents of a `<noscript>` inside `#main` are parsed as real
//! elements and survive the swap. The settings shells carried
//! `<noscript><style>.lc-set-nav{display:none}...</style></noscript>` in their
//! body: after a boosted navigation that style went live, hid the tab rail and
//! collapsed the settings column into the 13rem nav track until a reload.
//! `<head>` is never swapped, so the fallback lives there and nowhere else.

use std::path::{Path, PathBuf};

const ALLOWED: &[&str] = &["templates/base.html"];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir templates") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("html") {
            out.push(path);
        }
    }
}

#[test]
fn noscript_only_in_base_head() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(&root.join("templates"), &mut files);
    assert!(!files.is_empty(), "no templates found; walker broken");

    let mut hits = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let body = std::fs::read_to_string(f).expect("read template");
        if body.to_ascii_lowercase().contains("<noscript") {
            hits.push(rel);
        }
    }
    assert!(
        hits.is_empty(),
        "<noscript> outside base.html <head> goes live after a boosted swap: {hits:?}"
    );

    let base = std::fs::read_to_string(root.join("templates/base.html")).unwrap();
    let head_end = base.find("</head>").expect("base.html has </head>");
    assert!(
        base[head_end..].find("<noscript").is_none(),
        "base.html's <noscript> must sit inside <head>"
    );
}
