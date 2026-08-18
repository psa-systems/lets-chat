//! LC-746: every interactive form control in a template resolves an accessible
//! name. A screen reader announces an unnamed `<input>` / `<select>` /
//! `<textarea>` by its `placeholder` where there is one (not a name, and it
//! disappears the moment the user types) and by nothing at all where there is
//! not.
//!
//! A name comes from `aria-label`, `aria-labelledby`, a `<label for>` pointing
//! at the control's `id`, or a `<label>` wrapping it. That last one needs the
//! element ranges, which is why this sweep is a test rather than a line-regex
//! rule in ci-build/check-ui-conventions.nu (where the h1 and `<time>` rules of
//! the same issue live).
//!
//! Email templates are excluded: they render in a mail client with no form.

use std::path::{Path, PathBuf};

/// Controls whose name comes from elsewhere: `hidden` is not announced at all,
/// and `submit` / `button` / `reset` / `image` take their name from `value` or
/// `alt`.
const NAMELESS_TYPES: [&str; 5] = ["hidden", "submit", "button", "reset", "image"];

fn templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")
}

fn template_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![templates_dir()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {dir:?}: {e}")) {
            let p = entry.unwrap().path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) != Some("email") {
                    stack.push(p);
                }
            } else if p.extension().and_then(|e| e.to_str()) == Some("html") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Replace every comment, `<script>` and `<style>` span with spaces, keeping
/// newlines so byte offsets and line numbers still line up. A `<input>` named in
/// a comment or in a JS selector is documentation or a query, not markup, and
/// several templates do exactly that.
fn blank_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        let close: &[u8] = if bytes[i..].starts_with(b"{#") {
            b"#}"
        } else if bytes[i..].starts_with(b"<!--") {
            b"-->"
        } else if bytes[i..].starts_with(b"<script") {
            b"</script>"
        } else if bytes[i..].starts_with(b"<style") {
            b"</style>"
        } else {
            i += 1;
            continue;
        };
        let rest = &bytes[i..];
        let end = find(rest, close)
            .map(|p| i + p + close.len())
            .unwrap_or(bytes.len());
        for b in out[i..end].iter_mut() {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        i = end;
    }
    String::from_utf8(out).expect("blanking keeps the input's UTF-8 boundaries")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// End offset (exclusive) of the tag opening at `start`, honouring quoted
/// attribute values so a `>` inside one does not end the tag early.
fn tag_end(src: &str, start: usize) -> usize {
    let b = src.as_bytes();
    let mut quote = 0u8;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            c if quote != 0 => {
                if c == quote {
                    quote = 0;
                }
            }
            c @ (b'"' | b'\'') => quote = c,
            b'>' => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Byte ranges covered by a `<label>...</label>` element. Labels are not nested
/// anywhere in this template set, so a flat forward scan is exact.
fn label_ranges(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = find(&b[i..], b"<label") {
        let start = i + rel;
        let end = find(&b[start..], b"</label>")
            .map(|p| start + p + "</label>".len())
            .unwrap_or(b.len());
        out.push((start, end));
        i = end;
    }
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let at = tag.find(&needle)? + needle.len();
    let rest = &tag[at..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn has_attr(tag: &str, name: &str) -> bool {
    tag.contains(&format!(" {name}=\"")) || tag.contains(&format!(" {name}>"))
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].matches('\n').count() + 1
}

#[test]
fn every_form_control_has_an_accessible_name() {
    let root = templates_dir();
    let mut unnamed: Vec<String> = Vec::new();

    for path in template_files() {
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let src = blank_comments(&raw);
        let labels = label_ranges(&src);
        let label_targets: Vec<&str> = labels
            .iter()
            .filter_map(|&(s, _)| attr(&src[s..tag_end(&src, s)], "for"))
            .collect();

        for open in ["<input", "<select", "<textarea"] {
            let mut i = 0;
            while let Some(rel) = find(&src.as_bytes()[i..], open.as_bytes()) {
                let start = i + rel;
                let end = tag_end(&src, start);
                let tag = &src[start..end];
                i = end;

                let ty = attr(tag, "type").unwrap_or("");
                if NAMELESS_TYPES.contains(&ty) {
                    continue;
                }
                // `title` is deliberately not accepted: the spec treats it as a
                // last-resort name, and a tooltip is not reachable by touch.
                let named = has_attr(tag, "aria-label")
                    || has_attr(tag, "aria-labelledby")
                    || attr(tag, "id").is_some_and(|id| label_targets.contains(&id))
                    || labels.iter().any(|&(s, e)| start > s && end <= e);
                if !named {
                    let rel_path = path.strip_prefix(&root).unwrap_or(&path);
                    unnamed.push(format!(
                        "{}:{}: {}",
                        rel_path.display(),
                        line_of(&src, start),
                        tag.split_whitespace().collect::<Vec<_>>().join(" ")
                    ));
                }
            }
        }
    }

    assert!(
        unnamed.is_empty(),
        "form controls with no accessible name ({}). Add `aria-label`, or an \
         sr-only `<label for>` where voice control should reach it too:\n{}",
        unnamed.len(),
        unnamed.join("\n")
    );
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn comments_are_blanked_and_line_numbers_survive() {
        let src = "a\n{# <input>\n#}\n<script>\nq('<input>')\n</script>\n<input>";
        let out = blank_comments(src);
        assert_eq!(out.lines().count(), src.lines().count());
        assert!(!out[..out.find("<input>").unwrap()].contains("input"));
    }

    #[test]
    fn tag_end_skips_a_quoted_angle_bracket() {
        let src = r#"<input value="a > b" id="x"> tail"#;
        assert_eq!(&src[..tag_end(src, 0)], r#"<input value="a > b" id="x">"#);
    }

    #[test]
    fn wrapping_label_names_its_control() {
        let src = "<label>Name <input name=\"n\"></label>";
        let ranges = label_ranges(src);
        assert_eq!(ranges.len(), 1);
        let start = src.find("<input").unwrap();
        assert!(ranges.iter().any(|&(s, e)| start > s && start < e));
    }
}
