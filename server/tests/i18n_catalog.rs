//! LC-100: i18n catalog integrity (the CI gate).
//!
//! 1. Every translation key referenced in a template (`"key"|t` / `"key"|tn(`)
//!    must be defined in the English source catalog.
//! 2. Every non-English locale must define exactly the same message ids as
//!    English (full coverage - no missing or stray keys).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Message ids defined in a `.ftl` file (lines like `key = value`; comments and
/// continuations ignored). Ids are `[A-Za-z][A-Za-z0-9_-]*`.
fn ftl_keys(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Skip comments, blanks, and attribute/continuation lines.
        if trimmed.starts_with('#') || trimmed.is_empty() || line.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some((lhs, _)) = line.split_once('=') {
            let id = lhs.trim();
            if !id.is_empty()
                && id.starts_with(|c: char| c.is_ascii_alphabetic())
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                keys.insert(id.to_string());
            }
        }
    }
    keys
}

/// Union of message ids across every `.ftl` file in a locale dir (Fluent
/// merges them all at load time, so the catalog can be split by area).
fn locale_keys(lang: &str) -> BTreeSet<String> {
    let dir = manifest(&format!("locales/{lang}"));
    let mut keys = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {dir:?}: {e}")) {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) == Some("ftl") {
            keys.extend(ftl_keys(&p));
        }
    }
    keys
}

/// All keys referenced via the `t` / `tn` filters across every template.
fn referenced_keys() -> BTreeSet<String> {
    let dir = manifest("templates");
    let mut keys = BTreeSet::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("html") {
                let text = std::fs::read_to_string(&p).unwrap();
                collect_keys(&text, &mut keys);
            }
        }
    }
    keys
}

/// Pull `"<key>"|t` and `"<key>"|tn(` references out of template text. The
/// no-space pipe is the project convention (Askama parses `"x" | t` as bitor).
fn collect_keys(text: &str, out: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(rel) = text[i..].find("\"|t") {
        let close = i + rel; // index of the closing quote
                             // Walk back to the opening quote.
        if let Some(openrel) = text[..close].rfind('"') {
            let key = &text[openrel + 1..close];
            // After `"|t` must come `}`, ` `, `n`, or `(` - i.e. the `t`/`tn`
            // filter, not some other `...|t...` substring.
            let after = bytes.get(close + 3).copied();
            let is_filter = matches!(after, Some(b'n') | Some(b' ') | Some(b'}') | Some(b'('))
                || after.is_none();
            if is_filter
                && !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                out.insert(key.to_string());
            }
        }
        i = close + 3;
    }
}

#[test]
fn every_referenced_key_exists_in_english() {
    let en = locale_keys("en");
    let referenced = referenced_keys();
    let missing: Vec<&String> = referenced.iter().filter(|k| !en.contains(*k)).collect();
    assert!(
        missing.is_empty(),
        "templates reference keys missing from locales/en/main.ftl: {missing:?}"
    );
}

#[test]
fn locales_have_full_coverage_against_english() {
    let en = locale_keys("en");
    // Grows as locales are added (LC-188); a single entry today is fine.
    #[allow(clippy::single_element_loop)]
    for locale in ["es"] {
        let other = locale_keys(locale);
        let missing: Vec<&String> = en.difference(&other).collect();
        let stray: Vec<&String> = other.difference(&en).collect();
        assert!(
            missing.is_empty(),
            "locale {locale} is missing keys vs English: {missing:?}"
        );
        assert!(
            stray.is_empty(),
            "locale {locale} has keys not in English: {stray:?}"
        );
    }
}
