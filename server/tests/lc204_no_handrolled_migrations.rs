//! LC-204: structural drift-prevention for test migration setup.
//!
//! Every integration-test pool MUST be built via the drift-immune
//! `common::auth_pool()` / `common::chat_pool()` / `common::settings_pool()`
//! helpers (which run the FULL migration set via `sqlx::migrate!`, so a new
//! migration lands in every test automatically). The anti-pattern this ban
//! prevents is the hand-rolled `include_str!("../migrations/...)` list: it
//! pins a test to a specific migration subset that silently goes stale every
//! time a migration is added, surfacing later as "no such column" 500s in
//! whatever test touches the missing column. That drift was the LC-204 bug;
//! 35 files had accumulated it.
//!
//! ## Allow-list = the load-bearing-partial registry
//!
//! Unlike LC-152's exception-free no-bypass ban, an entry HERE is a
//! DECLARATION, not a bypass: "this file is a migration-BEHAVIOR test that
//! deliberately asserts the effect of a specific migration subset, so the
//! full-migration helper would change what it tests." A file on this list
//! MUST carry a top-of-file comment justifying why (see
//! `migration_enclaves.rs`). A future file that legitimately needs a partial
//! list gets added here WITH that justification - the ban forces the
//! justification to be explicit rather than letting silent drift back in.

use std::path::{Path, PathBuf};

/// FULL PATHS (relative to the server crate root), not basenames. A basename
/// match would let a future `tests/foo/migration_enclaves.rs` silently
/// inherit the exemption; the full path pins the exemption to exactly one
/// file.
const ALLOWED_PATHS: &[&str] = &["tests/migration_enclaves.rs"];

/// Anchored literal - the opening quote AND the trailing slash are part of
/// the pattern, so it cannot false-positive on `include_str!("../migrations_
/// test_data/...)` or any sibling directory that merely starts with
/// "migrations". Assembled via `concat!` so THIS file's source never
/// contains the full literal verbatim (otherwise the ban would flag
/// itself).
fn banned_pattern() -> String {
    concat!("include_str!(\"../", "migrations/").to_string()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir tests") {
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
fn no_handrolled_migration_lists_in_tests() {
    let mut files = Vec::new();
    walk(Path::new("tests"), &mut files);
    assert!(
        !files.is_empty(),
        "no .rs files under tests/ - must run from the server crate root"
    );

    let pat = banned_pattern();
    let allowed: Vec<PathBuf> = ALLOWED_PATHS.iter().map(PathBuf::from).collect();
    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        if allowed.iter().any(|p| file == p) {
            continue;
        }
        let body = std::fs::read_to_string(file).expect("read file");
        for (n, line) in body.lines().enumerate() {
            // Skip comment lines: a doc comment (this file's siblings, or an
            // explanatory note) may legitimately mention the banned pattern.
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains(&pat) {
                violations.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }

    if !violations.is_empty() {
        let list = violations.join("\n");
        panic!(
            "\n\nLC-204: hand-rolled migration list(s) found in tests/.\n\
             \n\
             Build the pool via `common::auth_pool()` / `common::chat_pool()` /\n\
             `common::settings_pool()` instead - they run the full migration set\n\
             via `sqlx::migrate!` and never go stale. A hand-rolled\n\
             `include_str!(\"../migrations/...)` list pins a stale schema subset\n\
             and surfaces later as \"no such column\" 500s.\n\
             \n\
             If a file is a deliberate migration-BEHAVIOR test (asserts a\n\
             specific migration's effect), add its full path to ALLOWED_PATHS\n\
             in this file WITH a justifying top-of-file comment (see\n\
             migration_enclaves.rs).\n\
             \n\
             Offending lines:\n{list}\n"
        );
    }
}

/// Meta-test: the allow-list isn't a defanged no-op. Every allowed file must
/// (a) exist on disk at that exact path, and (b) actually contain the banned
/// pattern - otherwise the exemption is dead and a future cleanup that
/// "removes the rot" from the allow-listed file would not be caught when it
/// then re-adds drift elsewhere. Same shape as LC-152's grep-ban sanity test.
#[test]
fn allow_list_entries_exist_and_are_load_bearing() {
    let pat = banned_pattern();
    for path in ALLOWED_PATHS {
        let body = std::fs::read_to_string(path).unwrap_or_else(|_| {
            panic!("ALLOWED_PATHS entry {path:?} not found on disk; if the file moved or was converted, remove it from the allow-list")
        });
        assert!(
            body.contains(&pat),
            "{path:?} is allow-listed but contains no hand-rolled migration list; \
             it no longer needs the exemption - remove it from ALLOWED_PATHS"
        );
    }
}
