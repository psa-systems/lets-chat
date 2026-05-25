//! LC-77 pre-refactor baseline: pin the rendered HTML of a webhook-authored
//! message under the current LC-74 `author_is_webhook` boolean shape.
//!
//! The MessageActor enum refactor in commit 1B replaces `author_is_webhook` +
//! `webhook_avatar_url` with a sum type. After the refactor, this test still
//! produces the same rendered HTML byte-for-byte (or DOM-for-DOM in the
//! fallback). The fixture files committed alongside this test are the
//! load-bearing baseline that proves the refactor was behavior-preserving for
//! LC-74 webhooks.
//!
//! Two cases:
//!   - Webhook without an avatar URL: renders initials fallback (template
//!     line 22, `room/message.html`).
//!   - Webhook with an avatar URL: renders `<img src="...">` (template
//!     line 20).
//!
//! Generation mode: set `FIXTURE_WRITE=1` to (re)write the fixture files.
//! Verification mode (default): reads the committed fixture, asserts the
//! rendered HTML matches byte-for-byte. If the byte compare fails the
//! test prints a unified diff snippet so the divergence is obvious.

use askama::Template;
use lets_chat::views::room::MessageView;
use lets_chat::views::ws_fragments::NewMessageFragment;

const FIXTURE_DIR: &str = "tests/fixtures";

fn webhook_view(with_avatar: bool) -> MessageView {
    MessageView {
        id: 1,
        room_id: 1,
        // LC-74 convention: webhook-authored rows store an empty user_id.
        user_id: String::new(),
        // The "username" field carries the webhook display name for webhook
        // posts (set by resolve_msg_author when webhook_id is Some).
        username: "Test Webhook".to_string(),
        display_name: None,
        avatar_ext: None,
        status: "active".to_string(),
        custom_status: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        edited_at: None,
        body: "hello from a webhook".to_string(),
        reactions: vec![],
        can_edit: false,
        can_delete: false,
        viewer_id: "00000000-0000-0000-0000-000000000001".to_string(),
        seen_caption: None,
        is_follow_up: false,
        show_unread_divider: false,
        reply_count: 0,
        parent_id: None,
        attachments: vec![],
        mentions: vec![],
        is_pinned: false,
        is_bookmarked: false,
        custom_emojis: vec![],
        quote_preview: None,
        is_system: false,
        poll: None,
        author_is_bot: false,
        author_is_webhook: true,
        webhook_avatar_url: if with_avatar {
            Some("https://example.com/avatar.png".to_string())
        } else {
            None
        },
    }
}

fn render_webhook_fragment(view: &MessageView) -> String {
    NewMessageFragment { message: view }
        .render()
        .expect("template render must succeed under fixture conditions")
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(FIXTURE_DIR).join(name)
}

fn assert_fixture_matches(name: &str, actual: &str) {
    let path = fixture_path(name);
    if std::env::var("FIXTURE_WRITE").is_ok() {
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
    let html = render_webhook_fragment(&view);
    assert_fixture_matches("lc77_webhook_render_no_avatar.html", &html);
}

#[test]
fn webhook_render_with_avatar_matches_fixture() {
    let view = webhook_view(true);
    let html = render_webhook_fragment(&view);
    assert_fixture_matches("lc77_webhook_render_with_avatar.html", &html);
}
