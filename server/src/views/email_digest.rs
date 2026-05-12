//! View structs for the missed-mentions/DMs email digest (phase 22 task 3).
//!
//! Two parallel Askama templates render the same logical content as the
//! two parts of a `multipart/alternative` email: one HTML, one plaintext.
//! Both forms see the same `DigestDmSection`/`DigestRoomSection` data and
//! choose which pre-rendered snippet (`snippet_plain` vs `snippet_html`)
//! to embed.
//!
//! The digest tick (task 4) builds these structs and renders both
//! templates into a single `EmailMessage`. Nothing in this file talks to
//! the database; it is pure presentation.

use askama::Template;

/// Top-level data for the HTML half of the digest.
#[derive(Template)]
#[template(path = "email/digest.html")]
pub struct DigestHtml<'a> {
    pub server_url: &'a str,
    pub dm_sections: &'a [DigestDmSection],
    pub room_sections: &'a [DigestRoomSection],
    /// Items dropped past the per-digest cap. Rendered as a "... and N
    /// more" footer when non-zero; suppressed when zero.
    pub overflow_count: usize,
}

/// Top-level data for the plaintext half. Mirrors `DigestHtml` so the
/// tick code can build one set of section structs and feed both
/// templates without re-deriving anything.
#[derive(Template)]
#[template(path = "email/digest.txt")]
pub struct DigestText<'a> {
    pub server_url: &'a str,
    pub dm_sections: &'a [DigestDmSection],
    pub room_sections: &'a [DigestRoomSection],
    pub overflow_count: usize,
}

/// One DM thread's worth of unread messages, addressed to a single peer.
/// Multiple `DigestItem`s in `items` are ordered oldest-first within the
/// thread so they read like the in-app scroll.
pub struct DigestDmSection {
    pub peer_username: String,
    /// `users.id` of the peer. Used to build the deep link back into the
    /// app: `<server_url>/dm/<peer_id>`.
    pub peer_id: String,
    pub items: Vec<DigestItem>,
}

/// One room's worth of unread mentions for this recipient. Items are
/// oldest-first within the room.
pub struct DigestRoomSection {
    pub room_name: String,
    pub room_id: i64,
    pub items: Vec<DigestItem>,
}

/// A single message inside a section. Both snippet forms are
/// pre-rendered by `email::digest::build_snippet`; the templates pick
/// the right one without doing any escaping themselves.
pub struct DigestItem {
    pub message_id: i64,
    pub author: String,
    /// Human-friendly timestamp, e.g. "Mon 14:23" or "Fri 09:08". The
    /// tick formats it from `messages.created_at` before constructing
    /// this struct so the templates stay free of date logic.
    pub created_at: String,
    /// Plaintext snippet (no HTML). Goes into the `.txt` template.
    pub snippet_plain: String,
    /// Pre-rendered HTML snippet (entities escaped, mentions bolded,
    /// URLs linkified). Embedded into the `.html` template via the
    /// `|safe` filter since it is already HTML.
    pub snippet_html: String,
    /// Absolute URL back into the app pointing at the specific message
    /// anchor: `<server_url>/room/<id>#m<message_id>` for room mentions,
    /// or `<server_url>/dm/<peer_id>#m<message_id>` for DM messages.
    pub deep_link: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item(author: &str, ts: &str, plain: &str, link: &str) -> DigestItem {
        DigestItem {
            message_id: 1,
            author: author.into(),
            created_at: ts.into(),
            snippet_plain: plain.into(),
            // Pre-rendered HTML snippet that the template embeds via |safe.
            // Keep it simple: real callers go through email::digest::build_snippet.
            snippet_html: format!("<strong>{author}</strong> said {plain}"),
            deep_link: link.into(),
        }
    }

    #[test]
    fn html_renders_dms_then_rooms_with_deep_links() {
        let dms = vec![DigestDmSection {
            peer_username: "bob".into(),
            peer_id: "user-bob".into(),
            items: vec![sample_item(
                "bob",
                "Mon 14:00",
                "hello there",
                "https://example.com/dm/user-bob#m1",
            )],
        }];
        let rooms = vec![DigestRoomSection {
            room_name: "general".into(),
            room_id: 42,
            items: vec![sample_item(
                "alice",
                "Mon 15:00",
                "ping",
                "https://example.com/room/42#m2",
            )],
        }];
        let tpl = DigestHtml {
            server_url: "https://example.com",
            dm_sections: &dms,
            room_sections: &rooms,
            overflow_count: 0,
        };
        let html = tpl.render().expect("html render");
        // Section headers must appear and DMs must come before mentions.
        let dm_pos = html.find("Direct messages").expect("dm header");
        let room_pos = html.find("Mentions").expect("room header");
        assert!(dm_pos < room_pos, "DMs should precede mentions");
        // Peer and room names rendered.
        assert!(html.contains("From bob"));
        assert!(html.contains("#general"));
        // Deep links present.
        assert!(html.contains("https://example.com/dm/user-bob#m1"));
        assert!(html.contains("https://example.com/room/42#m2"));
        // The pre-rendered HTML snippet is embedded via |safe (no
        // double-escape).
        assert!(html.contains("<strong>bob</strong> said hello there"));
        // The overflow footer is suppressed when overflow_count is 0.
        assert!(!html.contains("and 0 more"));
    }

    #[test]
    fn html_omits_dm_section_when_empty() {
        let rooms = vec![DigestRoomSection {
            room_name: "general".into(),
            room_id: 1,
            items: vec![sample_item("alice", "Mon", "hi", "https://x/r/1#m1")],
        }];
        let tpl = DigestHtml {
            server_url: "https://x",
            dm_sections: &[],
            room_sections: &rooms,
            overflow_count: 0,
        };
        let html = tpl.render().unwrap();
        assert!(
            !html.contains("Direct messages"),
            "DM header should not render when no DM sections"
        );
        assert!(html.contains("Mentions"));
    }

    #[test]
    fn html_overflow_footer_appears_when_count_positive() {
        let tpl = DigestHtml {
            server_url: "https://x",
            dm_sections: &[],
            room_sections: &[],
            overflow_count: 12,
        };
        let html = tpl.render().unwrap();
        assert!(html.contains("12 more"), "expected overflow line: {html}");
    }

    #[test]
    fn plaintext_mirrors_html_content_without_markup() {
        let dms = vec![DigestDmSection {
            peer_username: "bob".into(),
            peer_id: "user-bob".into(),
            items: vec![sample_item(
                "bob",
                "Mon 14:00",
                "hello there",
                "https://example.com/dm/user-bob#m1",
            )],
        }];
        let tpl = DigestText {
            server_url: "https://example.com",
            dm_sections: &dms,
            room_sections: &[],
            overflow_count: 0,
        };
        let txt = tpl.render().expect("text render");
        // No HTML markup.
        assert!(
            !txt.contains("<"),
            "plaintext should not contain '<': {txt}"
        );
        assert!(!txt.contains("&amp;"), "plaintext should not html-escape");
        // The plain snippet is rendered literally (not the html one).
        assert!(txt.contains("hello there"));
        assert!(txt.contains("https://example.com/dm/user-bob#m1"));
        assert!(txt.contains("Direct messages"));
    }
}
