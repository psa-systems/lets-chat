//! LC-948: `docs/api.md` and `docs/protocol-bridges.md` together are the
//! published contract for the `/api/v1` bearer-token API. This test asserts
//! every path template mounted by `routes::api::router()` is mentioned in at
//! least one of the two docs, so a new route cannot ship undocumented.

use regex::Regex;
use std::fs;
use std::path::Path;

/// Normalize a path template so `{room_id}`, `{bridge_id}`, and `{id}` are
/// treated as the same "some path parameter" placeholder: the docs are not
/// required to use the same parameter name as the code.
fn normalize(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                "{}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn documented_paths(markdown: &str) -> Vec<String> {
    let re = Regex::new(r"/api/v1[A-Za-z0-9_/{}-]*").unwrap();
    re.find_iter(markdown)
        .map(|m| normalize(m.as_str()))
        .collect()
}

#[test]
fn docs_cover_every_api_route() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let api_md = fs::read_to_string(root.join("docs/api.md")).expect("read docs/api.md");
    let bridges_md = fs::read_to_string(root.join("docs/protocol-bridges.md"))
        .expect("read docs/protocol-bridges.md");

    let mut documented = documented_paths(&api_md);
    documented.extend(documented_paths(&bridges_md));

    for path in lets_chat::routes::api::ROUTE_PATHS {
        let normalized = normalize(path);
        assert!(
            documented.iter().any(|d| d == &normalized),
            "route {path} (normalized: {normalized}) is not documented in \
             docs/api.md or docs/protocol-bridges.md"
        );
    }
}
