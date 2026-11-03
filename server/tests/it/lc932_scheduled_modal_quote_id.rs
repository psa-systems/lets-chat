//! LC-932: the scheduled-send modal rebuilds its own `URLSearchParams`
//! instead of submitting the composer form (see scheduled_modal.html's
//! header comment for why), so nothing at compile time keeps its posted
//! field names in sync with `routes::scheduled::CreateForm`. `quote_id`
//! was the field that drifted: the server grew support for it but the
//! modal's JS never learned to read or send it.
//!
//! This test pins both sides: every field the modal's submit handler sets
//! on `fd` must also be a field on `CreateForm`, and the required set
//! `{room_id, body, scheduled_for, quote_id, file_id, repeat}` must appear
//! in both.

use regex::Regex;
use std::collections::HashSet;

const REQUIRED: &[&str] = &[
    "room_id",
    "body",
    "scheduled_for",
    "quote_id",
    "file_id",
    "repeat",
];

fn create_form_fields() -> HashSet<String> {
    let src = std::fs::read_to_string("src/routes/scheduled.rs").expect("read scheduled.rs");
    let struct_start = src
        .find("pub struct CreateForm")
        .expect("CreateForm struct not found in scheduled.rs");
    let body_start = src[struct_start..]
        .find('{')
        .expect("CreateForm opening brace")
        + struct_start;
    let body_end = src[body_start..]
        .find('}')
        .expect("CreateForm closing brace")
        + body_start;
    let body = &src[body_start..body_end];

    let field_re = Regex::new(r"pub\s+(\w+)\s*:").unwrap();
    field_re
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .collect()
}

fn modal_posted_fields() -> HashSet<String> {
    let html = std::fs::read_to_string("templates/scheduled_modal.html")
        .expect("read scheduled_modal.html");
    let set_re = Regex::new(r#"fd\.set\('(\w+)'"#).unwrap();
    set_re
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .collect()
}

#[test]
fn scheduled_modal_field_set_is_superset_of_required_and_all_exist_on_create_form() {
    let form_fields = create_form_fields();
    let posted_fields = modal_posted_fields();

    assert!(
        !form_fields.is_empty(),
        "found no `pub` fields on CreateForm - the test's struct scan is broken"
    );
    assert!(
        !posted_fields.is_empty(),
        "found no fd.set(...) calls in scheduled_modal.html - the test's scan is broken"
    );

    for &name in REQUIRED {
        assert!(
            posted_fields.contains(name),
            "scheduled_modal.html's submit handler never posts `{name}`; required set is {REQUIRED:?}"
        );
        assert!(
            form_fields.contains(name),
            "routes::scheduled::CreateForm has no `{name}` field; required set is {REQUIRED:?}"
        );
    }
}
