//! LC-77 render fixture pins.
//!
//! The MessageActor enum (commit 1B) replaced the original LC-74
//! `author_is_webhook` boolean + `webhook_avatar_url` pair with a sum
//! type. The fixture files committed alongside this test are the
//! load-bearing baseline that proved the refactor was behavior-preserving
//! for LC-74 webhooks; they also pin the new LC-77 EmailInbox arm so a
//! future change cannot silently break the third actor's render shape.
//!
//! Cases (each with and without an avatar URL):
//!   - Webhook (LC-74).
//!   - EmailInbox (LC-77).
//!
//! Generation mode: set `FIXTURE_WRITE=1` to (re)write the fixture files.
//! Verification mode (default): reads the committed fixture, asserts the
//! rendered HTML matches byte-for-byte. If the byte compare fails the
//! test prints a unified diff snippet so the divergence is obvious.

use askama::Template;
use lets_chat::views::message_actor::MessageActor;
use lets_chat::views::room::MessageView;
use lets_chat::views::ws_fragments::NewMessageFragment;

const FIXTURE_DIR: &str = "tests/fixtures";

fn synthetic_view(actor: MessageActor, username: &str, body: &str) -> MessageView {
    MessageView {
        id: 1,
        room_id: 1,
        // Synthetic-actor convention: user_id is empty.
        user_id: String::new(),
        // The "username" field carries the synthetic actor's display name
        // (set by resolve_msg_author when webhook_id or email_inbox_id is Some).
        username: username.to_string(),
        display_name: None,
        avatar_ext: None,
        status: "active".to_string(),
        custom_status: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        edited_at: None,
        body: body.to_string(),
        reactions: vec![],
        can_edit: false,
        can_delete: false,
        viewer_id: "00000000-0000-0000-0000-000000000001".to_string(),
        seen_caption: None,
        is_follow_up: false,
        show_unread_divider: false,
        day_label: None,
        shame_enabled: false,
        shame_hidden: None,
        reply_count: 0,
        parent_id: None,
        attachments: vec![],
        mentions: vec![],
        is_pinned: false,
        is_bookmarked: false,
        ack: None,
        custom_emojis: vec![],
        quote_preview: None,
        suppress_quote_preview: false,
        is_system: false,
        poll: None,
        follow_up: None,
        author_is_bot: false,
        actor,
        channels: vec![],
    }
}

fn webhook_view(with_avatar: bool) -> MessageView {
    let avatar = with_avatar.then(|| "https://example.com/avatar.png".to_string());
    synthetic_view(
        MessageActor::Webhook(avatar),
        "Test Webhook",
        "hello from a webhook",
    )
}

fn email_inbox_view(with_avatar: bool) -> MessageView {
    let avatar = with_avatar.then(|| "https://example.com/avatar.png".to_string());
    synthetic_view(
        MessageActor::EmailInbox(avatar),
        "Test Inbox",
        "hello from an email inbox",
    )
}

fn render_fragment(view: &MessageView) -> String {
    // LC-230: client_id is None here, so the rendered wrapper is byte-identical
    // to the pre-LC-230 shape and the existing fixtures stay valid.
    NewMessageFragment {
        message: view,
        client_id: None,
    }
    .render()
    .expect("template render must succeed under fixture conditions")
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(FIXTURE_DIR).join(name)
}

/// Whether the caller asked to regenerate the golden files.
///
/// LC-605: this used to be `env::var("FIXTURE_WRITE").is_ok()`, which treats a
/// variable that is *present but empty* as opt-in. `dev/cargo` forwards
/// `-e "FIXTURE_WRITE=${FIXTURE_WRITE:-}"`, so under the standard local wrapper
/// the variable was always present and always empty - every local run silently
/// rewrote the fixtures and passed. The check only did its job under a bare
/// `cargo test`, i.e. nowhere anyone actually ran it. Require a meaningful
/// value so the default is "assert".
fn fixture_write_requested() -> bool {
    std::env::var("FIXTURE_WRITE")
        .map(|v| {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

fn assert_fixture_matches(name: &str, actual: &str) {
    let path = fixture_path(name);
    if fixture_write_requested() {
        std::fs::write(&path, actual)
            .unwrap_or_else(|e| panic!("write fixture {}: {e}", path.display()));
        eprintln!("wrote fixture {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read fixture {}: {e}. Run with FIXTURE_WRITE=1 to regenerate.",
            path.display()
        )
    });
    if actual == expected {
        return;
    }
    // Byte-diff failed. Print a compact diff so the failure is diagnosable.
    eprintln!("---- expected ({}) ----", path.display());
    eprintln!("{expected}");
    eprintln!("---- actual ----");
    eprintln!("{actual}");
    panic!(
        "fixture {} diverged. If the refactor changed whitespace only, \
         consider a DOM-equal fallback; if intentional, regenerate with \
         FIXTURE_WRITE=1.",
        path.display()
    );
}

#[test]
fn webhook_render_no_avatar_matches_fixture() {
    let view = webhook_view(false);
    let html = render_fragment(&view);
    assert_fixture_matches("lc77_webhook_render_no_avatar.html", &html);
}

#[test]
fn webhook_render_with_avatar_matches_fixture() {
    let view = webhook_view(true);
    let html = render_fragment(&view);
    assert_fixture_matches("lc77_webhook_render_with_avatar.html", &html);
}

#[test]
fn email_inbox_render_no_avatar_matches_fixture() {
    let view = email_inbox_view(false);
    let html = render_fragment(&view);
    assert_fixture_matches("lc77_email_inbox_render_no_avatar.html", &html);
}

#[test]
fn email_inbox_render_with_avatar_matches_fixture() {
    let view = email_inbox_view(true);
    let html = render_fragment(&view);
    assert_fixture_matches("lc77_email_inbox_render_with_avatar.html", &html);
}
