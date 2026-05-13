use askama::Template;

use crate::db;
use crate::db::mentions::MentionRef;
use crate::models::custom_emoji::EmojiRef;
use crate::models::{Attachment, Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::markdown;

pub struct MessageView {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub body: String,
    pub reactions: Vec<ReactionView>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub viewer_id: String,
    /// HH:MM peer-read timestamp shown under this message in a DM, or None.
    /// Only one own-authored message in a DM should have this set at a time.
    pub seen_caption: Option<String>,
    /// True when this message follows another message from the same author
    /// within MESSAGE_GROUPING_WINDOW. Hides the username/timestamp header.
    pub is_follow_up: bool,
    /// True for the first message in the page that the viewer has not yet
    /// read on this server. Renders an "Unread messages" divider above the
    /// message and tells the auto-scroll script to anchor here on load.
    pub show_unread_divider: bool,
    /// Count of non-deleted thread replies rooted at this message. Zero
    /// suppresses the "N replies" pill. Only meaningful for top-level
    /// messages; replies always render with `reply_count = 0`.
    pub reply_count: i64,
    /// `Some(N)` when this view represents a thread reply (rendered inside
    /// the panel). `None` for top-level messages in the main feed.
    pub parent_id: Option<i64>,
    /// File attachments linked to this message. Empty for plain text
    /// messages; the template only renders attachment markup when non-empty.
    pub attachments: Vec<Attachment>,
    /// Resolved `@username` mentions in the body. Used by `body_html()` to
    /// render mention chips inline. Empty for messages with no resolved
    /// mentions (e.g. DMs, which don't write mention rows).
    pub mentions: Vec<MentionRef>,
    /// True when this message currently has a row in `pinned_messages`.
    /// Drives the Pin / Unpin flip in the hover menu. Page-render call
    /// sites populate this from a single bulk lookup; per-message render
    /// sites (post, edit, delete broadcasts) look it up individually.
    pub is_pinned: bool,
    /// True when the viewing user has bookmarked this message. Drives the
    /// Save / Unsave flip in the hover menu. Per-viewer state, so the
    /// page render path looks it up in bulk for the current viewer and
    /// per-message renders query it for the specific viewer.
    pub is_bookmarked: bool,
    /// Custom emoji set in the message's enclave, used by `body_html()` to
    /// rewrite `:shortcode:` tokens into `<img>` tags. Empty for DM messages
    /// and any non-enclave room; unknown shortcodes pass through as text.
    pub custom_emojis: Vec<EmojiRef>,
    /// `Some(...)` when this message quote-replies to an earlier message;
    /// renders as an inline chip above the body that links back to the
    /// original. `None` for plain top-level messages.
    pub quote_preview: Option<QuotePreview>,
}

/// Inline preview of the message a quote-reply is responding to. Rendered
/// above the reply's own body. Carries only what's needed to draw the chip;
/// the full message is reachable via `id` for clients that want to scroll
/// to it. `author_label` is the original author's display name (or
/// username), and `body_excerpt` is a plain-text, already-HTML-escaped
/// snippet truncated to a render-friendly length. `deleted` lets the
/// template render a stub when the quoted row was soft-deleted between
/// the reply being written and being displayed.
#[derive(Debug, Clone)]
pub struct QuotePreview {
    pub id: i64,
    pub author_label: String,
    pub body_excerpt: String,
    pub deleted: bool,
}

impl MessageView {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }

    /// True when the message has no body text and exactly one image
    /// attachment. The template renders this as an unbubbled image (LC-3).
    pub fn is_image_only(&self) -> bool {
        self.body.trim().is_empty() && self.attachments.len() == 1 && self.attachments[0].is_image()
    }

    /// Render the body as CommonMark markdown with fenced code blocks
    /// syntax-highlighted. Inline text segments still flow through the chat
    /// pipeline: `@username` mentions become chips, `:shortcode:` tokens
    /// become custom-emoji images, and bare URLs are linkified. The template
    /// renders the result with `|safe`.
    pub fn body_html(&self) -> String {
        markdown::render(&self.body, &self.mentions, &self.custom_emojis)
    }

    /// First URL in the body, or `None`. The template uses this to render
    /// at most one preview-card lazy-load shell per message so repeated
    /// links in a single body don't multiply network fetches.
    pub fn first_url(&self) -> Option<String> {
        let finder = linkify::LinkFinder::new();
        finder
            .links(&self.body)
            .find(|l| matches!(l.kind(), linkify::LinkKind::Url))
            .map(|l| l.as_str().to_string())
    }

    /// Percent-encoded form of `first_url()` for safe injection into the
    /// `hx-get` query string. Askama 0.12 has no urlencode built-in, so the
    /// view layer pre-encodes.
    pub fn first_url_encoded(&self) -> Option<String> {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        self.first_url()
            .map(|u| utf8_percent_encode(&u, NON_ALPHANUMERIC).to_string())
    }
}

pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn linkify_body(
    body: &str,
    emoji_map: &std::collections::HashMap<&str, &EmojiRef>,
) -> String {
    let finder = linkify::LinkFinder::new();
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    for link in finder.links(body) {
        if !matches!(link.kind(), linkify::LinkKind::Url) {
            continue;
        }
        let start = link.start();
        let end = link.end();
        if start > cursor {
            emojify_and_escape(&body[cursor..start], emoji_map, &mut out);
        }
        let url = link.as_str();
        out.push_str("<a href=\"");
        out.push_str(&html_escape(url));
        out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\" class=\"text-blue-600 hover:underline\">");
        out.push_str(&html_escape(url));
        out.push_str("</a>");
        cursor = end;
    }
    if cursor < body.len() {
        emojify_and_escape(&body[cursor..], emoji_map, &mut out);
    }
    out
}

/// Render the `:shortcode:` syntax for custom emojis as `<img>` tags inside
/// the supplied plain-text segment. Text outside any matched token is
/// HTML-escaped; unknown shortcodes are emitted as literal escaped text.
pub(crate) fn emojify_and_escape(
    s: &str,
    emoji_map: &std::collections::HashMap<&str, &EmojiRef>,
    out: &mut String,
) {
    if emoji_map.is_empty() {
        out.push_str(&html_escape(s));
        return;
    }
    use std::sync::OnceLock;
    static EMOJI_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = EMOJI_RE
        .get_or_init(|| regex::Regex::new(r":([a-z0-9_]{2,32}):").expect("valid emoji regex"));
    let mut cursor = 0usize;
    for cap in re.captures_iter(s) {
        let whole = cap.get(0).unwrap();
        let shortcode = cap.get(1).unwrap().as_str();
        let Some(emoji) = emoji_map.get(shortcode) else {
            continue;
        };
        if whole.start() > cursor {
            out.push_str(&html_escape(&s[cursor..whole.start()]));
        }
        out.push_str(&render_emoji_img(emoji));
        cursor = whole.end();
    }
    if cursor < s.len() {
        out.push_str(&html_escape(&s[cursor..]));
    }
}

/// HTML for a single inline custom emoji. Inline width keeps the bubble
/// height stable; `alt` text preserves the typed token for screen readers
/// and copy-paste.
pub(crate) fn render_emoji_img(emoji: &EmojiRef) -> String {
    format!(
        r#"<img class="custom-emoji inline-block h-5 w-5 align-text-bottom" src="/api/emojis/{id}" alt=":{code}:" title=":{code}:">"#,
        id = emoji.id,
        code = html_escape(&emoji.shortcode),
    )
}

/// Render `body` to HTML in three passes:
/// 1. Substitute `@username` tokens (boundary-anchored, same shape as the
///    server-side mention parser) that resolve to a known mention with a
///    chip anchor that links to the user's profile. Tokens that don't
///    resolve are emitted as escaped literal text.
/// 2. Linkify URL runs in plain-text segments. Mention substitution runs
///    BEFORE linkify so `@foo.com` doesn't accidentally become a URL.
/// 3. Concatenate.
///
/// All output is HTML-escaped at the segment level; the chip and anchor
/// markup is the only inserted HTML, and mentioned usernames are escaped
/// before inclusion.
pub(crate) fn render_body(body: &str, mentions: &[MentionRef], emojis: &[EmojiRef]) -> String {
    use std::collections::HashMap;
    let emoji_map: HashMap<&str, &EmojiRef> =
        emojis.iter().map(|e| (e.shortcode.as_str(), e)).collect();
    // No early-return on empty mentions: `@here` / `@channel` render as
    // broadcast chips without needing a MentionRef, so the loop must run
    // even when no user mentions are resolved.
    let lookup: HashMap<String, &MentionRef> = mentions
        .iter()
        .map(|m| (m.username.to_ascii_lowercase(), m))
        .collect();
    // Boundary-anchored mention regex matches the server-side parser shape.
    // We capture the leading boundary character so we can preserve
    // whitespace/start when rebuilding the string.
    use std::sync::OnceLock;
    static MENTION_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = MENTION_RE
        .get_or_init(|| regex::Regex::new(r"(^|\s)@([A-Za-z0-9_-]{1,32})").expect("valid regex"));

    let mut out = String::with_capacity(body.len() * 2);
    let mut cursor = 0usize;
    for cap in re.captures_iter(body) {
        let whole = cap.get(0).unwrap();
        let token = cap.get(2).unwrap();
        let token_lower = token.as_str().to_ascii_lowercase();
        // Broadcast tokens render as chips without consulting the mentions
        // lookup. The resolver step has already written mention rows for the
        // resolved users; here we just turn the literal `@here` / `@channel`
        // in the body text into a span. No profile link because there is no
        // user behind the token. Same color as a user mention so the recipient
        // is not distracted by visual differentiation; v1 treats both as one
        // mention class.
        if token_lower == "here" || token_lower == "channel" {
            let lead = cap.get(1).unwrap();
            if whole.start() > cursor {
                out.push_str(&linkify_body(&body[cursor..whole.start()], &emoji_map));
            }
            out.push_str(&html_escape(lead.as_str()));
            out.push_str("<span class=\"font-medium text-blue-700 bg-blue-50 rounded px-1\">@");
            out.push_str(&html_escape(&token_lower));
            out.push_str("</span>");
            cursor = whole.end();
            continue;
        }
        if let Some(m) = lookup.get(&token_lower) {
            // Emit text up to the boundary char.
            let lead = cap.get(1).unwrap();
            if whole.start() > cursor {
                out.push_str(&linkify_body(&body[cursor..whole.start()], &emoji_map));
            }
            out.push_str(&html_escape(lead.as_str()));
            out.push_str("<a href=\"/profile/");
            out.push_str(&html_escape(&m.user_id));
            out.push_str("\" class=\"font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded px-1\">@");
            out.push_str(&html_escape(&m.username));
            out.push_str("</a>");
            cursor = whole.end();
        }
        // Unknown token: leave for the trailing linkify pass to escape.
    }
    if cursor < body.len() {
        out.push_str(&linkify_body(&body[cursor..], &emoji_map));
    }
    out
}

pub struct ReactionView {
    pub emoji: String,
    pub count: i64,
    pub viewer_reacted: bool,
    /// Pre-rendered display HTML for the emoji glyph. Custom emojis become
    /// `<img>` tags; unicode emojis become the escaped text glyph. The
    /// template renders this with `|safe`.
    pub display_html: String,
}

impl ReactionView {
    /// Build a ReactionView with the display HTML pre-rendered against the
    /// supplied enclave emoji set. Pass an empty slice in DM contexts (or
    /// any room without an enclave); unknown shortcodes fall through to
    /// escaped text so the bar still renders.
    pub fn new(emoji: String, count: i64, viewer_reacted: bool, emojis: &[EmojiRef]) -> Self {
        let display_html = Self::render_emoji(&emoji, emojis);
        Self {
            emoji,
            count,
            viewer_reacted,
            display_html,
        }
    }

    /// Render an emoji string for the reaction bar. Custom emoji shortcodes
    /// have shape `:foo:` and are looked up in the supplied set; anything
    /// else (unicode glyphs, unknown shortcodes) falls back to escaped text.
    pub fn render_emoji(emoji: &str, emojis: &[EmojiRef]) -> String {
        if let Some(code) = strip_shortcode(emoji) {
            for e in emojis {
                if e.shortcode == code {
                    return render_emoji_img(e);
                }
            }
        }
        html_escape(emoji)
    }
}

/// Max characters of the quoted body shown inline in a quote-reply chip.
/// Past this point we append an ellipsis so the chip stays a single, scan-
/// able line in dense message lists.
const QUOTE_EXCERPT_MAX_CHARS: usize = 140;

/// Resolve a single `quote_id` to a [`QuotePreview`] for rendering. Returns
/// `Ok(None)` when the referenced row has been hard-deleted (the FK
/// `ON DELETE SET NULL` removes the column value rather than the row), and
/// `Ok(Some(deleted: true))` when the row exists but is soft-deleted so the
/// chip renders as a "(message deleted)" stub.
///
/// Use [`build_quote_previews_bulk`] when rendering many messages at once
/// to avoid N+1 lookups.
pub async fn build_quote_preview(
    chat_pool: &sqlx::SqlitePool,
    auth_pool: &sqlx::SqlitePool,
    quote_id: i64,
) -> Result<Option<QuotePreview>, sqlx::Error> {
    let map = build_quote_previews_bulk(chat_pool, auth_pool, &[quote_id]).await?;
    Ok(map.into_iter().next().map(|(_, v)| v))
}

/// Bulk variant of [`build_quote_preview`]. Issues one `SELECT ... IN (...)`
/// for the message rows and one batched display-name lookup. The returned
/// map is keyed by `quote_id` (the id of the quoted message, NOT the
/// quoting message) so callers can index it directly from `RawMessage.quote_id`.
pub async fn build_quote_previews_bulk(
    chat_pool: &sqlx::SqlitePool,
    auth_pool: &sqlx::SqlitePool,
    quote_ids: &[i64],
) -> Result<std::collections::HashMap<i64, QuotePreview>, sqlx::Error> {
    use sqlx::Row;
    use std::collections::HashMap;

    let mut deduped: Vec<i64> = quote_ids.to_vec();
    deduped.sort_unstable();
    deduped.dedup();
    if deduped.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = std::iter::repeat("?")
        .take(deduped.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql =
        format!("SELECT id, user_id, body, deleted_at FROM messages WHERE id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in &deduped {
        q = q.bind(id);
    }
    let rows = q.fetch_all(chat_pool).await?;

    let mut user_ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("user_id")).collect();
    user_ids.sort();
    user_ids.dedup();
    let user_id_refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
    let name_map = db::auth::display_names_for_ids(auth_pool, &user_id_refs).await?;

    let mut out: HashMap<i64, QuotePreview> = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.get("id");
        let user_id: String = row.get("user_id");
        let body: String = row.get("body");
        let deleted_at: Option<String> = row.get("deleted_at");
        let author_label = name_map
            .get(&user_id)
            .map(|(username, display)| {
                display
                    .as_deref()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or(username.as_str())
                    .to_string()
            })
            .unwrap_or_else(|| "(unknown)".to_string());
        let excerpt = if deleted_at.is_some() {
            String::new()
        } else {
            excerpt_for_quote(&body)
        };
        out.insert(
            id,
            QuotePreview {
                id,
                author_label,
                body_excerpt: excerpt,
                deleted: deleted_at.is_some(),
            },
        );
    }
    Ok(out)
}

/// Collapse a message body to a single-line plain-text excerpt suitable for
/// inline display inside a quote-reply chip. Newlines become spaces and the
/// result is truncated to `QUOTE_EXCERPT_MAX_CHARS` at a char boundary with
/// an ellipsis appended. The result is plain text - HTML escaping happens
/// in the template via Askama's default auto-escape, so this function does
/// not pre-escape (which would double-encode).
fn excerpt_for_quote(body: &str) -> String {
    let flat = body.replace('\r', " ").replace('\n', " ");
    let trimmed = flat.trim();
    let mut out = String::new();
    let mut count = 0usize;
    for c in trimmed.chars() {
        if count >= QUOTE_EXCERPT_MAX_CHARS {
            out.push('\u{2026}');
            break;
        }
        out.push(c);
        count += 1;
    }
    out
}

/// Return the inner shortcode when `s` has shape `:foo:` with a valid charset.
/// Returns None otherwise so the caller can fall back to text rendering.
pub fn strip_shortcode(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.len() < 4 || bytes[0] != b':' || bytes[bytes.len() - 1] != b':' {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    if inner.len() < 2 || inner.len() > 32 {
        return None;
    }
    if !inner
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    Some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_collapses_newlines_and_trims() {
        let out = excerpt_for_quote("  hi\nthere\r\nfriend  ");
        assert_eq!(out, "hi there  friend");
    }

    #[test]
    fn excerpt_truncates_with_ellipsis_past_limit() {
        let long = "a".repeat(QUOTE_EXCERPT_MAX_CHARS + 10);
        let out = excerpt_for_quote(&long);
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars.len(), QUOTE_EXCERPT_MAX_CHARS + 1);
        assert_eq!(chars.last(), Some(&'\u{2026}'));
    }

    #[test]
    fn excerpt_leaves_html_unescaped_for_template_to_escape() {
        // Pre-escaping here would double-encode because Askama auto-escapes.
        let out = excerpt_for_quote("<b>bold</b>");
        assert_eq!(out, "<b>bold</b>");
    }

    #[test]
    fn render_body_emits_chip_and_link_without_double_escape() {
        let mentions = vec![MentionRef {
            user_id: "alice-id".into(),
            username: "alice".into(),
        }];
        let out = render_body("hey @alice check https://example.com", &mentions, &[]);
        // The chip is a real anchor (not escaped).
        assert!(
            out.contains(r#"<a href="/profile/alice-id" class="font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded px-1">@alice</a>"#),
            "missing mention chip: {out}"
        );
        // The URL is linkified as a real anchor too.
        assert!(
            out.contains(r#"<a href="https://example.com""#)
                && out.contains(r#">https://example.com</a>"#),
            "missing URL link: {out}"
        );
        // No escaped angle brackets anywhere.
        assert!(!out.contains("&lt;a"), "anchor was escaped: {out}");
        assert!(!out.contains("&gt;@alice"), "chip text was escaped: {out}");
    }

    #[test]
    fn render_body_leaves_unknown_token_as_text() {
        let out = render_body("ping @nobody", &[], &[]);
        // Unknown tokens fall through to linkify+escape: literal `@nobody`.
        assert!(out.contains("@nobody"), "unknown token lost: {out}");
        assert!(!out.contains("href=\"/profile/"));
    }

    #[test]
    fn render_body_emits_broadcast_chip_without_mention_lookup() {
        // @here and @channel are rendered unconditionally as broadcast chips:
        // no MentionRef entry needed, no profile link. This is what makes the
        // chip appear in the body even though resolved mention rows carry the
        // real users' usernames (not the broadcast token).
        let out = render_body("hey @here and @channel quick update", &[], &[]);
        assert!(
            out.contains(
                r#"<span class="font-medium text-blue-700 bg-blue-50 rounded px-1">@here</span>"#
            ),
            "missing @here chip: {out}"
        );
        assert!(
            out.contains(
                r#"<span class="font-medium text-blue-700 bg-blue-50 rounded px-1">@channel</span>"#
            ),
            "missing @channel chip: {out}"
        );
        // Broadcast chips have no profile link.
        assert!(
            !out.contains(r#"href="/profile/here""#) && !out.contains(r#"href="/profile/channel""#),
            "broadcast token rendered with profile link: {out}"
        );
    }

    #[test]
    fn render_body_broadcast_chip_is_case_insensitive() {
        // The mention parser is case-insensitive in its token-lookup pass
        // (note the `to_ascii_lowercase` in render_body). Mirror that for
        // broadcast tokens so `@Here` and `@CHANNEL` also render as chips.
        let out = render_body("attn @Here and @CHANNEL", &[], &[]);
        assert!(
            out.contains(">@here</span>"),
            "@Here did not normalize: {out}"
        );
        assert!(
            out.contains(">@channel</span>"),
            "@CHANNEL did not normalize: {out}"
        );
    }

    #[test]
    fn render_body_chips_broadcast_tokens_in_dm_body() {
        // DM bodies go through the same renderer as room bodies; the renderer
        // has no room-context parameter and intentionally does not branch on
        // room type. Posting `@here` in a DM writes no mention rows and fires
        // no broadcast events (the DM gate in the resolver path skips them),
        // but the chip still renders as text styling. Phase 21 decision:
        // "option A" - chip everywhere; a DM is implicitly one-on-one and the
        // chip-without-effect is honest about what was typed.
        let out = render_body("can we sync @here?", &[], &[]);
        assert!(
            out.contains(">@here</span>"),
            "DM body did not chip @here (option A regression): {out}"
        );
    }

    #[test]
    fn render_body_email_does_not_become_broadcast_chip() {
        // `foo@here.example.com` is an email-shaped string with `here` after
        // `@`. The boundary regex prevents it from matching, same as for
        // user tokens. Guard against a regression where someone "simplifies"
        // the broadcast branch and drops the boundary check.
        let out = render_body("contact foo@here.example.com", &[], &[]);
        assert!(
            !out.contains(">@here</span>"),
            "email became broadcast chip: {out}"
        );
    }

    #[test]
    fn render_body_does_not_treat_email_as_mention() {
        let mentions = vec![MentionRef {
            user_id: "bar-id".into(),
            username: "bar".into(),
        }];
        // Even though `bar` is a known mention, `foo@bar.com` has no
        // boundary before `@`, so the chip pass must skip it. The text
        // should appear as a plain string (linkified as part of any URL,
        // but `foo@bar.com` is not a URL by linkify rules).
        let out = render_body("ping foo@bar.com", &mentions, &[]);
        assert!(
            !out.contains("href=\"/profile/"),
            "email matched chip: {out}"
        );
        assert!(out.contains("foo@bar.com"), "plain text lost: {out}");
    }

    #[test]
    fn render_body_substitutes_known_custom_emoji() {
        let emojis = vec![EmojiRef {
            id: 42,
            shortcode: "party".into(),
        }];
        let out = render_body("hello :party: world", &[], &emojis);
        assert!(out.contains("src=\"/api/emojis/42\""), "no img: {out}");
        assert!(out.contains("alt=\":party:\""), "no alt: {out}");
        // Surrounding plain text is preserved and escaped.
        assert!(out.contains("hello "), "leading text lost: {out}");
        assert!(out.contains(" world"), "trailing text lost: {out}");
    }

    #[test]
    fn render_body_leaves_unknown_shortcode_as_text() {
        let out = render_body("plain :missing: text", &[], &[]);
        // Unknown shortcode passes through as literal escaped text.
        assert!(out.contains(":missing:"), "shortcode lost: {out}");
    }

    #[test]
    fn render_body_does_not_substitute_emoji_inside_url() {
        let emojis = vec![EmojiRef {
            id: 1,
            shortcode: "foo".into(),
        }];
        let out = render_body("see https://example.com/:foo:.png ok", &[], &emojis);
        // The url is fully linkified - the :foo: inside the path should not
        // become an img tag because the linkify pass owns that range.
        assert!(
            !out.contains("src=\"/api/emojis/1\""),
            "emoji leaked into url: {out}"
        );
        assert!(
            out.contains("href=\"https://example.com/:foo:.png\""),
            "url lost: {out}"
        );
    }

    #[test]
    fn strip_shortcode_validates() {
        assert_eq!(strip_shortcode(":ok:"), Some("ok"));
        assert_eq!(strip_shortcode(":party_42:"), Some("party_42"));
        assert_eq!(strip_shortcode("👍"), None);
        assert_eq!(strip_shortcode(":Bad:"), None);
        assert_eq!(strip_shortcode(":a:"), None);
        assert_eq!(strip_shortcode("foo"), None);
        assert_eq!(strip_shortcode(":no-dash:"), None);
    }
}

#[derive(Template)]
#[template(path = "room/page.html")]
pub struct RoomPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
    /// Viewer's per-room mute mode for this room. Drives the notify
    /// dropdown's selected radio and button label.
    pub mute_mode: &'a str,
    /// Pre-rendered pinned-strip HTML (or empty string when there are
    /// zero pins). Built by the route handler from `db::pinned` so the
    /// template just inlines the markup verbatim with `|safe`.
    pub pinned_strip_html: String,
}

#[derive(Template)]
#[template(path = "room/composer.html")]
pub struct ComposerFragment<'a> {
    pub room: &'a Room,
}

#[derive(Template)]
#[template(path = "room/edit_form.html")]
pub struct EditFormFragment<'a> {
    pub message_id: i64,
    pub current_body: &'a str,
}

#[derive(Template)]
#[template(path = "room/message.html")]
pub struct SingleMessageFragment<'a> {
    pub message: &'a MessageView,
    pub oob: bool,
}

#[derive(Template)]
#[template(path = "partials/reaction_bar.html")]
pub struct ReactionBarFragment<'a> {
    pub message_id: i64,
    pub reactions: &'a [ReactionView],
}

/// Right-side thread drawer. Replaces `#thread-panel` outerHTML when opened.
#[derive(Template)]
#[template(path = "room/thread_panel.html")]
pub struct ThreadPanelFragment<'a> {
    pub room: &'a Room,
    pub parent: &'a MessageView,
    pub replies: &'a [MessageView],
}

/// Empty thread panel container, used to close the drawer.
#[derive(Template)]
#[template(path = "room/thread_panel_closed.html")]
pub struct ThreadPanelClosedFragment;

/// Single reply rendered inside the panel's reply list.
#[derive(Template)]
#[template(path = "room/thread_reply.html")]
pub struct ThreadReplyFragment<'a> {
    pub message: &'a MessageView,
}

/// "N replies" pill rendered under a top-level message. Standalone fragment
/// so the WS layer can OOB-swap it when reply_count changes.
#[derive(Template)]
#[template(path = "room/reply_count.html")]
pub struct ReplyCountFragment {
    pub message_id: i64,
    pub room_id: i64,
    pub reply_count: i64,
    pub oob: bool,
}

/// Chip that drops into `#composer-quote-bar` when the user clicks "Quote"
/// on a message. Carries the hidden `quote_id` input that the composer's
/// next submit picks up alongside the body and any file_id.
#[derive(Template)]
#[template(path = "room/composer_quote_chip.html")]
pub struct ComposerQuoteChip {
    pub quote_id: i64,
    pub author_label: String,
    pub body_excerpt: String,
}
