//! LC-206: structural no-bypass enforcement for image decoding.
//!
//! Every image decode in `server/src/` MUST go through the shared safe
//! helpers `crate::uploads::pipeline::process_image_safely()` /
//! `process_preview_safely()` (which run the sync decode + re-encode on the
//! blocking pool and fold a decoder panic into `SafeImageError::Panic` via
//! the `JoinError` boundary). The audit that motivated this ticket found the
//! four existing decode sites were ALREADY each doing their own
//! `spawn_blocking(|| process_image(...))` + JoinError handling inline - so
//! the threat was never a missing panic boundary (tokio's `JoinError`
//! already catches under `panic=unwind`), it was four copies of the same
//! boilerplate and nothing stopping a fifth caller from decoding raw on the
//! async thread. This ban retires that duplication structurally.
//!
//! What this test forbids in `src/` outside `pipeline.rs`:
//!
//! 1. **Raw decoder entry points** (`ImageReader::open/new`,
//!    `GifDecoder::new`, `image::open/load`, `image::io::Reader`). A new
//!    decode site must live in `pipeline.rs` behind a helper, not scattered.
//! 2. **`run_blocking`** - the `#[doc(hidden)] pub` panic-boundary seam.
//!    Same posture as LC-152 banning `outbound_unchecked`: a `src/` caller
//!    invoking `run_blocking(|| process_image(...))` directly would
//!    replicate the spawn_blocking + JoinError boilerplate inline and defeat
//!    the dedup the two `*_safely` helpers exist to provide. The seam is for
//!    the killer test (in `tests/`, not walked here) only.
//!
//! Exclusion list: EXACTLY `src/uploads/pipeline.rs` - the one module that
//! owns every decoder call and the helpers. Full-path match, not basename,
//! so a future `src/foo/pipeline.rs` can't silently inherit the allowance.
//! The sync `process_image` / `preview_from_path` stay `pub` as the
//! pure-pipeline unit-test seam (see `uploads_pipeline.rs`); visibility is
//! NOT the enforcement here, this grep-ban is - same as LC-152 keeps raw
//! `reqwest` construction visible inside `http_client.rs`.

use std::path::{Path, PathBuf};

/// Full paths relative to the server crate root, not basenames.
const ALLOWED_FILE_PATHS: &[&str] = &["src/uploads/pipeline.rs"];

const FORBIDDEN: &[(&str, &str)] = &[
    ("ImageReader::open", "raw ImageReader::open()"),
    ("ImageReader::new", "raw ImageReader::new()"),
    ("GifDecoder::new", "raw GifDecoder::new()"),
    ("image::open(", "image::open() shortcut"),
    ("image::load", "image::load* shortcut"),
    ("image::io::Reader", "image::io::Reader decoder"),
    (
        "run_blocking",
        "run_blocking() panic-boundary seam in src/ (bypasses the *_safely helpers)",
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
fn no_raw_image_decode_in_src() {
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
            // Skip comment lines: a doc/line comment may legitimately mention
            // a forbidden pattern when explaining what's forbidden.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for (pat, label) in FORBIDDEN {
                if line.contains(pat) {
                    violations.push(format!(
                        "{}:{}: forbidden ({label})\n      | {}",
                        file.display(),
                        n + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    if !violations.is_empty() {
        let body = violations.join("\n");
        panic!(
            "\n\nLC-206 grep-ban violations.\n\
             \n\
             Every image decode in server/src/ must go through\n\
             `crate::uploads::pipeline::process_image_safely()` or\n\
             `process_preview_safely()`. A raw decoder entry point, or a\n\
             direct `run_blocking()` call, outside src/uploads/pipeline.rs\n\
             replicates the spawn_blocking + JoinError boilerplate the helpers\n\
             centralize (LC-206 existed to retire exactly that duplication).\n\
             \n\
             Offending sites:\n{body}\n"
        );
    }
}

/// Sanity test for the test itself (LC-152 / LC-204 meta-test shape). Proves
/// the allow-listed file exists at that exact relative path AND actually
/// contains a forbidden pattern - so the exception is load-bearing, not dead
/// code that would let a future gutting of pipeline.rs pass unnoticed.
#[test]
fn allow_list_actually_finds_the_allowed_file() {
    for path in ALLOWED_FILE_PATHS {
        let body = std::fs::read_to_string(path).unwrap_or_else(|_| {
            panic!("ALLOWED_FILE_PATHS entry {path:?} not found on disk; if the file moved, update the ban")
        });
        let has_raw = FORBIDDEN.iter().any(|(pat, _)| body.contains(pat));
        assert!(
            has_raw,
            "{path:?} contains no forbidden pattern; either it was gutted (bug) \
             or the FORBIDDEN list is stale (refresh it)"
        );
    }
}
