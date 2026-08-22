//! LC-152: structural no-bypass enforcement.
//!
//! Every outbound reqwest call in `server/src/` MUST go through
//! `crate::http_client::outbound_*()`. The exception-free rule is what
//! makes the LC-152 SSRF fix structural rather than audited-compliant; a
//! single bypass is a complete SSRF (the audit that motivated this PR
//! found exactly that - Web Push at `push/mod.rs` had NO SSRF guard at
//! all because nothing prevented constructing a raw `reqwest::Client`).
//!
//! What this test enforces:
//!
//! 1. **Raw `reqwest::Client` construction is forbidden in src/.** Catches
//!    a future PR that adds a new outbound site and forgets the helper.
//! 2. **`outbound_unchecked` is forbidden in src/.** This is the
//!    `#[doc(hidden)]` no-filter test seam. A `src/` site calling it gets
//!    an unfiltered client without tripping the raw-construction ban (the
//!    call is a "blessed" helper, not a raw constructor). That's an
//!    unfiltered-client escape hatch living inside the safe module. Ban it
//!    in src/ exactly as hard as raw construction.
//!
//! Exclusion list: EXACTLY `http_client.rs`. No broad glob, no `tests/`
//! pattern (this file lives in `tests/` which isn't walked at all). If
//! the exception list ever grows, it's a red flag - the design intent is
//! a single chokepoint module.

use std::path::{Path, PathBuf};

/// Exception list - **full paths relative to the server crate root**, not
/// basenames. A basename match would silently exempt any future file
/// named `http_client.rs` anywhere under `src/` (e.g.,
/// `src/foo/http_client.rs`), inheriting the raw-construction allowance
/// and reopening the bypass. Full-path match means exactly ONE file is
/// exempted, and the exception list cannot be widened by adding files
/// with the same basename.
const ALLOWED_FILE_PATHS: &[&str] = &["src/http_client.rs"];

const FORBIDDEN: &[(&str, &str)] = &[
    ("reqwest::Client::new", "raw reqwest::Client::new()"),
    ("reqwest::Client::builder", "raw reqwest::Client::builder()"),
    ("reqwest::ClientBuilder", "raw reqwest::ClientBuilder"),
    ("reqwest::get(", "reqwest::get() shortcut"),
    (
        "reqwest::blocking::",
        "reqwest::blocking::* (same SSRF posture)",
    ),
    (
        "outbound_unchecked",
        "outbound_unchecked() test-seam in src/ (unfiltered client escape hatch)",
    ),
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_raw_reqwest_or_unchecked_in_src() {
    let mut files = Vec::new();
    walk(Path::new("src"), &mut files);
    assert!(
        !files.is_empty(),
        "no .rs files found under src/ - test must run from the server crate root"
    );

    let allowed: Vec<PathBuf> = ALLOWED_FILE_PATHS.iter().map(PathBuf::from).collect();
    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        if allowed.iter().any(|p| file == p) {
            continue;
        }
        let body = std::fs::read_to_string(file).expect("read file");
        for (n, line) in body.lines().enumerate() {
            let trimmed = line.trim_start();
            // Skip line-comment-only and doc-comment lines: they may
            // legitimately mention the forbidden pattern when explaining
            // what's forbidden (this test's siblings, for example).
            if trimmed.starts_with("//") {
                continue;
            }
            for (pat, label) in FORBIDDEN {
                if line.contains(pat) {
                    violations.push(format!(
                        "{}:{}: forbidden ({label})\n      | {line}",
                        file.display(),
                        n + 1,
                    ));
                }
            }
        }
    }

    if !violations.is_empty() {
        let body = violations.join("\n");
        panic!(
            "\n\nLC-152 grep-ban violations.\n\
             \n\
             Every outbound reqwest call in server/src/ must go through\n\
             `crate::http_client::outbound_no_redirects()` or\n\
             `crate::http_client::outbound_following_redirects()`. Direct\n\
             reqwest::Client construction OR `outbound_unchecked()` outside\n\
             the helper module bypasses the LC-152 SSRF filter.\n\
             \n\
             The exception-free rule is the security boundary: see the audit\n\
             that motivated this PR (Web Push at push/mod.rs shipped\n\
             completely unguarded because nothing prevented raw client\n\
             construction).\n\
             \n\
             Offending sites:\n{body}\n"
        );
    }
}

/// Sanity test for the test itself. Two things this proves:
/// 1. Every entry in `ALLOWED_FILE_PATHS` exists on disk at that exact
///    relative path. Catches a file move that would otherwise silently
///    DEFANG the exception list (no file matches the entry, the ban
///    still runs across every file in `src/`, the test still passes
///    because nothing was wrongly exempted - but the EXCEPTION is dead
///    code, which would surface as a confusing compile error the next
///    time `http_client.rs` legitimately needs to use a forbidden
///    pattern). Better to fail loudly on a moved file.
/// 2. The allowed file ACTUALLY contains at least one forbidden pattern.
///    If it doesn't, either the file was gutted (the helper isn't doing
///    its job) or the `FORBIDDEN` list is stale.
#[test]
fn allow_list_actually_finds_the_allowed_file() {
    for path in ALLOWED_FILE_PATHS {
        let body = std::fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("ALLOWED_FILE_PATHS entry {path:?} not found on disk; if the file moved, update the ban"));
        let has_raw = FORBIDDEN.iter().any(|(pat, _)| body.contains(pat));
        assert!(
            has_raw,
            "{path:?} does not contain any forbidden patterns; either it has \
             been gutted (bug) or the FORBIDDEN list is stale (refresh it)"
        );
    }
}
