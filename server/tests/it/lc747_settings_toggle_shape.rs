//! LC-747: the shared on/off settings control renders one shape.
//!
//! `partials/settings_toggle.html` used to hardcode `/room/{room_id}/{field}`
//! as its post target, so it could only ever serve the room Manage page. It now
//! takes the URL and the form field name from the caller, which is what lets
//! any boolean setting reuse it. These assertions pin what the caller-supplied
//! values actually reach in the rendered HTML, which compiling the template
//! does not prove: a binding that resolves to the wrong URL still type-checks.
//!
//! The complementary static invariants (every `.lc-toggle` checkbox carries
//! `role="switch"`, and nothing outside this partial hand-rolls `.lc-switch`)
//! are enforced across all templates by `ci-build/check-boolean-settings.nu`.

use askama::Template;
use lets_chat::views::room_moderators::SettingsToggleFragment;
use std::path::Path;

fn fragment(action: &str, name: &'static str, enabled: bool) -> SettingsToggleFragment {
    SettingsToggleFragment {
        action: action.to_string(),
        name,
        enabled,
        aria_label: "Coyote Mode".into(),
        on_label: "On.".into(),
        on_text: "Bursts are auto-banned.".into(),
        off_label: "Off.".into(),
        off_text: "No burst detection.".into(),
        status: "Saved".into(),
    }
}

#[test]
fn posts_to_the_caller_supplied_action_and_field() {
    let html = fragment("/room/42/assistant", "enabled", false)
        .render()
        .expect("render");
    assert!(
        html.contains(r#"action="/room/42/assistant""#),
        "no-JS submit posts to the caller's URL: {html}"
    );
    assert!(
        html.contains(r#"hx-post="/room/42/assistant""#),
        "htmx submit posts to the same URL: {html}"
    );
    assert!(
        html.contains(r#"name="enabled""#),
        "the hidden next-value uses the caller's field name: {html}"
    );

    // A different surface gets a different URL and field out of the same partial.
    let other = fragment("/enclave/7/visibility", "is_public", false)
        .render()
        .expect("render");
    assert!(
        other.contains(r#"action="/enclave/7/visibility""#),
        "{other}"
    );
    assert!(other.contains(r#"name="is_public""#), "{other}");
}

#[test]
fn announces_as_a_switch_and_posts_the_opposite_value() {
    let on = fragment("/room/42/stage", "enabled", true)
        .render()
        .expect("render");
    assert!(on.contains(r#"role="switch""#), "{on}");
    assert!(on.contains(r#"aria-checked="true""#), "{on}");
    assert!(
        on.contains(r#"value="0""#),
        "an on switch submits the off value: {on}"
    );

    let off = fragment("/room/42/stage", "enabled", false)
        .render()
        .expect("render");
    assert!(off.contains(r#"aria-checked="false""#), "{off}");
    assert!(
        off.contains(r#"value="1""#),
        "an off switch submits the on value: {off}"
    );
}

#[test]
fn the_status_slot_is_always_present() {
    // The slot is what the global error net in settings.js writes a failed save
    // into, so it must exist even on the page render where it is empty.
    let html = fragment("/room/42/digest", "enabled", true)
        .render()
        .expect("render");
    assert!(
        html.contains(r#"class="lc-set-status" role="status" aria-live="polite""#),
        "{html}"
    );
}

#[test]
fn room_manage_binds_an_action_for_every_toggle() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/room/manage.html");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    // LC-855 added the per-room remote-control toggle alongside the original
    // three; every one still binds an action and goes through the shared partial.
    for field in ["assistant", "digest", "stage", "remote-control"] {
        assert!(
            text.contains(&format!(
                r#"{{% let action = "/room/{{}}/{field}"|format(room.id) %}}"#
            )),
            "room/manage.html must bind an action for the {field} toggle"
        );
    }
    assert_eq!(
        text.matches(r#"{% include "partials/settings_toggle.html" %}"#)
            .count(),
        4,
        "the four Manage toggles all go through the shared partial"
    );
}
