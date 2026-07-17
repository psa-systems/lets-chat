#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::db;
use crate::db::mentions::MentionRef;
use crate::models::custom_emoji::EmojiRef;
use crate::models::{Attachment, Room, User};
use crate::views::channel_complete::ChannelRef;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::markdown;
use crate::views::message_actor::MessageActor;

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
    /// LC-490: `Some(_)` when this message requires acknowledgement; carries the
    /// per-viewer ack roster state. `None` for ordinary messages. Full-bubble
    /// re-renders (edit/pin/bookmark/ack) must populate this or they would wipe
    /// the ack bar for a required message; `load_message_view_for_viewer`
    /// computes it centrally for the WS single-message paths.
    pub ack: Option<AckState>,
    /// Custom emoji set in the message's enclave, used by `body_html()` to
    /// rewrite `:shortcode:` tokens into `<img>` tags. Empty for DM messages
    /// and any non-enclave room; unknown shortcodes pass through as text.
    pub custom_emojis: Vec<EmojiRef>,
    /// LC-323: rooms in the message's enclave the viewer can `#`-link, used by
    /// `body_html()` to rewrite `#name` tokens into links to `/room/{id}`.
    /// Empty for DM / non-enclave messages; unknown tokens pass through as text.
    pub channels: Vec<ChannelRef>,
    /// `Some(...)` when this message quote-replies to an earlier message;
    /// renders as an inline chip above the body that links back to the
    /// original. `None` for plain top-level messages.
    pub quote_preview: Option<QuotePreview>,
    /// LC-462: when true, the inline "Replying to" stub is omitted because the
    /// quoted parent is the message rendered immediately above this one in the
    /// same view (Slack-style adjacency suppression). Only set by the main
    /// timeline list builder, which has previous-row context; every other
    /// construction site leaves it `false` (no adjacency known), so the stub
    /// renders as before for single-message / WS / thread renders.
    pub suppress_quote_preview: bool,
    /// True for server-authored system notices (e.g. "started a call").
    /// The template renders these as a centered, non-interactive line with
    /// no avatar, hover menu, reactions, or thread affordances.
    pub is_system: bool,
    /// LC-66: `Some(...)` when this message is a poll. Renders an
    /// interactive options block beneath the body (the question lives in
    /// the body). `None` for ordinary messages.
    pub poll: Option<PollView>,
    /// LC-527: `Some(...)` when this message anchors a follow-up task list.
    /// Renders an interactive checklist beneath the body. `None` otherwise.
    pub follow_up: Option<FollowUpView>,
    /// LC-73: true when the author is a bot. Renders a "bot" badge next to
    /// the username.
    pub author_is_bot: bool,
    /// LC-77: actor identity for branching the avatar + label affordances
    /// in `room/message.html`. Carries the LC-74 webhook avatar URL
    /// inline; the User variant is unit because the user-identity scalars
    /// already live on this struct.
    pub actor: MessageActor,
    /// LC-244: `Some(label)` renders a day separator row ("Today" /
    /// "Yesterday" / absolute date) ABOVE this message. Set by the page
    /// builder on the first message and on every UTC day change; `None`
    /// for the per-message WS fragments (the client inserts live dividers).
    pub day_label: Option<String>,
    /// LC-342: true when shame-tagging is enabled for this room's enclave, so
    /// the message renders the "Flag" control. False on every non-room-timeline
    /// surface (DM / thread / WS fragments) in the prototype.
    pub shame_enabled: bool,
    /// LC-342: `Some(_)` when the message is default-hidden by community tags or
    /// a moderator override; the body renders behind a "show anyway"
    /// click-through. `None` = shown normally.
    pub shame_hidden: Option<crate::db::shame_tags::HiddenState>,
}

/// LC-66: a poll rendered beneath its anchor message. Built by
/// [`build_poll_view`]; the template branches on `closed` and `anonymous`.
pub struct PollView {
    pub message_id: i64,
    pub allows_multi: bool,
    pub anonymous: bool,
    pub closed: bool,
    pub closes_at: Option<String>,
    pub total_voters: i64,
    pub options: Vec<PollOptionView>,
    /// LC-491: when set, render as an event card (RSVP). `event_at` is the
    /// start time (UTC), `event_location` an optional place.
    pub event_at: Option<String>,
    pub event_location: Option<String>,
}

pub struct PollOptionView {
    pub id: i64,
    pub text: String,
    pub count: i64,
    /// Percent of distinct voters who picked this option (0-100). Drives the
    /// result bar width.
    pub pct: i64,
    pub voted_by_me: bool,
    /// Display labels of voters, newest-first. Always empty for anonymous
    /// polls so identity never leaves the server.
    pub voters: Vec<String>,
}

/// Assemble a [`PollView`] for `message_id` as seen by `viewer_id`, or
/// `None` when the message is not a poll. Resolves voter display names from
/// the auth pool for non-anonymous polls only.
pub async fn build_poll_view(
    chat_pool: &sqlx::SqlitePool,
    auth_pool: &sqlx::SqlitePool,
    message_id: i64,
    viewer_id: &str,
) -> Result<Option<PollView>, sqlx::Error> {
    let Some(poll) = db::polls::get(chat_pool, message_id).await? else {
        return Ok(None);
    };
    let tallies = db::polls::options_with_counts(chat_pool, message_id).await?;
    let total_voters = db::polls::voter_count(chat_pool, message_id).await?;
    let mine = db::polls::user_votes(chat_pool, message_id, viewer_id).await?;
    let closed = db::polls::is_closed(chat_pool, message_id).await?;

    let mut options = Vec::with_capacity(tallies.len());
    for t in tallies {
        let voters = if poll.anonymous {
            Vec::new()
        } else {
            let ids = db::polls::voter_ids_for_option(chat_pool, t.id).await?;
            let mut names = Vec::with_capacity(ids.len());
            for id in ids {
                let label = db::auth::find_user_by_id(auth_pool, &id)
                    .await?
                    .map(|u| u.display_name.unwrap_or(u.username))
                    .unwrap_or_else(|| "(unknown)".to_string());
                names.push(label);
            }
            names
        };
        let pct = if total_voters > 0 {
            (t.count * 100 / total_voters).min(100)
        } else {
            0
        };
        options.push(PollOptionView {
            id: t.id,
            text: t.text,
            count: t.count,
            pct,
            voted_by_me: mine.contains(&t.id),
            voters,
        });
    }
    Ok(Some(PollView {
        message_id,
        allows_multi: poll.allows_multi,
        anonymous: poll.anonymous,
        closed,
        closes_at: poll.closes_at,
        total_voters,
        options,
        event_at: poll.event_at,
        event_location: poll.event_location,
    }))
}

/// LC-527: a follow-up task list rendered beneath its anchor message. Built by
/// [`build_follow_up_view`]; the template renders each item as a checkbox row
/// with a self-claim control. `done_count`/`total` drive the progress caption.
pub struct FollowUpView {
    pub message_id: i64,
    pub items: Vec<FollowUpItemView>,
    pub done_count: usize,
    pub total: usize,
}

pub struct FollowUpItemView {
    pub id: i64,
    pub text: String,
    pub done: bool,
    /// Display label of the current assignee, or `None` when unclaimed.
    pub assignee_label: Option<String>,
    /// True when the viewer is the assignee (drives the claim/release toggle).
    pub assigned_to_me: bool,
}

/// Assemble a [`FollowUpView`] for `message_id` as seen by `viewer_id`, or
/// `None` when the message is not a follow-up list. Resolves assignee display
/// names from the auth pool.
pub async fn build_follow_up_view(
    chat_pool: &sqlx::SqlitePool,
    auth_pool: &sqlx::SqlitePool,
    message_id: i64,
    viewer_id: &str,
) -> Result<Option<FollowUpView>, sqlx::Error> {
    if db::followups::get(chat_pool, message_id).await?.is_none() {
        return Ok(None);
    }
    let raw = db::followups::items(chat_pool, message_id).await?;
    let done_count = raw.iter().filter(|i| i.done).count();
    let total = raw.len();
    let mut items = Vec::with_capacity(total);
    for it in raw {
        let assigned_to_me = it.assignee_id.as_deref() == Some(viewer_id);
        let assignee_label = match it.assignee_id.as_deref() {
            None => None,
            Some(uid) => Some(
                db::auth::find_user_by_id(auth_pool, uid)
                    .await?
                    .map(|u| u.display_name.unwrap_or(u.username))
                    .unwrap_or_else(|| "(unknown)".to_string()),
            ),
        };
        items.push(FollowUpItemView {
            id: it.id,
            text: it.text,
            done: it.done,
            assignee_label,
            assigned_to_me,
        });
    }
    Ok(Some(FollowUpView {
        message_id,
        items,
        done_count,
        total,
    }))
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

/// LC-244: human label for the UTC calendar day of `created_at`
/// (`YYYY-MM-DD HH:MM:SS`). Returns the localized "Today"/"Yesterday" for
/// those days relative to UTC `now`, otherwise an English long-form date
/// (`June 9, 2026`). Day boundaries are UTC to match the app's UTC inline
/// timestamps; viewer-local boundaries and month-name localization are
/// deferred to a future timezone pass (LC-244). Falls back to the raw date
/// prefix if `created_at` is malformed.
pub(crate) fn day_label_for(created_at: &str) -> String {
    use chrono::{Duration, NaiveDate, Utc};
    let raw = created_at.get(..10).unwrap_or(created_at);
    let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") else {
        return raw.to_string();
    };
    let today = Utc::now().date_naive();
    if date == today {
        crate::i18n::translate_current("room-day-today")
    } else if date == today - Duration::days(1) {
        crate::i18n::translate_current("room-day-yesterday")
    } else {
        date.format("%B %-d, %Y").to_string()
    }
}

/// LC-314: convert a SQLite `YYYY-MM-DD HH:MM:SS` UTC timestamp to a JS-safe
/// RFC3339 (`YYYY-MM-DDTHH:MM:SSZ`). Non-standard input passes through.
/// LC-319: also reused by the status picker for the auto-clear expiry hint.
pub(crate) fn to_iso_utc(created_at: &str) -> String {
    let s = created_at.trim();
    if s.len() >= 19 && s.as_bytes().get(10) == Some(&b' ') {
        format!("{}T{}Z", &s[..10], &s[11..19])
    } else {
        s.to_string()
    }
}

impl MessageView {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }

    /// LC-244: the message's UTC calendar day (`YYYY-MM-DD`), the prefix of
    /// `created_at`. Rendered as `data-lc-day` so the client can insert a day
    /// divider above a live-arriving message that crosses a day boundary.
    pub fn day(&self) -> &str {
        self.created_at
            .get(..10)
            .unwrap_or(self.created_at.as_str())
    }

    /// LC-314: `created_at` as a JS-safe RFC3339 UTC string for the relative-
    /// time `<time datetime>` attribute. SQLite stores `YYYY-MM-DD HH:MM:SS`
    /// (UTC); `new Date("YYYY-MM-DD HH:MM:SS")` is parsed as LOCAL (or rejected)
    /// across browsers, so emit `YYYY-MM-DDTHH:MM:SSZ` which is unambiguously
    /// UTC. Any non-standard value passes through unchanged (best-effort).
    pub fn created_at_iso(&self) -> String {
        to_iso_utc(&self.created_at)
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
        markdown::render_with_channels(
            &self.body,
            &self.mentions,
            &self.custom_emojis,
            &self.channels,
        )
    }

    /// LC-434: inline HTML for a system / call-event body. These are
    /// server-authored, so the only markup is an optional single `[label](url)`
    /// link (the transcript-saved notice) with an internal `url`; everything
    /// else is escaped plain text. Rendering it inline (not through full
    /// markdown) keeps the one-line event row free of the block `<p>` wrapper
    /// and turns the transcript link into a real anchor instead of literal
    /// `[text](url)`.
    pub fn system_html(&self) -> String {
        render_system_inline(&self.body)
    }

    /// LC-284: true when this message directly `@`-mentions the viewer, so the
    /// template can highlight it. The resolved mention set is already loaded for
    /// chip rendering, so this is a cheap membership check (no extra query).
    pub fn mentions_viewer(&self) -> bool {
        self.mentions.iter().any(|m| m.user_id == self.viewer_id)
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

/// LC-434: render a server-authored system-event body inline. Recognizes a
/// single leading `[label](url)` markdown link with an internal (`/...`) `url`
/// and emits a real anchor; any other body is escaped plain text. Server-only
/// input, but the url is still gated to internal paths and both parts are
/// HTML-escaped so a future event body can never inject markup.
pub(crate) fn render_system_inline(body: &str) -> String {
    if let Some(rest) = body.strip_prefix('[') {
        if let Some(close) = rest.find("](") {
            let label = &rest[..close];
            let after = &rest[close + 2..];
            if let Some(end) = after.find(')') {
                let url = &after[..end];
                let trailing = &after[end + 1..];
                // Internal paths only ("/x", not "//host" or "javascript:").
                if url.starts_with('/') && !url.starts_with("//") {
                    return format!(
                        "<a href=\"{}\" class=\"lc-sysevent-link\">{}</a>{}",
                        html_escape(url),
                        html_escape(label),
                        html_escape(trailing),
                    );
                }
            }
        }
    }
    html_escape(body)
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
pub(crate) fn render_body(
    body: &str,
    mentions: &[MentionRef],
    emojis: &[EmojiRef],
    channels: &[ChannelRef],
) -> String {
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
    // LC-323: `#name` -> room link map, keyed by lowercased name.
    let channel_map: HashMap<String, &ChannelRef> = channels
        .iter()
        .map(|c| (c.name.to_ascii_lowercase(), c))
        .collect();
    // Boundary-anchored token regex. The `@` arm matches the server-side
    // mention parser shape; the `#` arm (LC-323) shares the same boundary
    // anchoring so it never fires mid-word or inside a URL fragment. The sigil
    // is captured so the loop can branch.
    use std::sync::OnceLock;
    static TOKEN_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = TOKEN_RE.get_or_init(|| {
        regex::Regex::new(r"(^|\s)([@#])([A-Za-z0-9_-]{1,64})").expect("valid regex")
    });

    let mut out = String::with_capacity(body.len() * 2);
    let mut cursor = 0usize;
    for cap in re.captures_iter(body) {
        let whole = cap.get(0).unwrap();
        let sigil = cap.get(2).unwrap().as_str();
        let token = cap.get(3).unwrap();
        let token_lower = token.as_str().to_ascii_lowercase();
        let lead = cap.get(1).unwrap();

        if sigil == "#" {
            // LC-323: resolve `#name` to a same-enclave room the viewer can
            // access. Unresolved tokens fall through to the trailing linkify
            // pass as literal text (no leak about inaccessible rooms).
            if let Some(c) = channel_map.get(&token_lower) {
                if whole.start() > cursor {
                    out.push_str(&linkify_body(&body[cursor..whole.start()], &emoji_map));
                }
                out.push_str(&html_escape(lead.as_str()));
                out.push_str("<a href=\"/room/");
                out.push_str(&c.room_id.to_string());
                out.push_str("\" class=\"font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded px-1\">#");
                out.push_str(&html_escape(&c.name));
                out.push_str("</a>");
                cursor = whole.end();
            }
            continue;
        }

        // sigil == "@"
        // Broadcast tokens render as chips without consulting the mentions
        // lookup. The resolver step has already written mention rows for the
        // resolved users; here we just turn the literal `@here` / `@channel`
        // in the body text into a span. No profile link because there is no
        // user behind the token. Same color as a user mention so the recipient
        // is not distracted by visual differentiation; v1 treats both as one
        // mention class.
        if token_lower == "here" || token_lower == "channel" {
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
    /// LC-266: comma-joined display names of everyone who reacted with this
    /// emoji, rendered as the pill's `title` (the "who reacted" tooltip).
    /// Empty when names could not be resolved (best-effort: no tooltip).
    pub reactors_title: String,
}

impl ReactionView {
    /// Build a ReactionView with the display HTML pre-rendered against the
    /// supplied enclave emoji set. Pass an empty slice in DM contexts (or
    /// any room without an enclave); unknown shortcodes fall through to
    /// escaped text so the bar still renders. `reactors_title` is the
    /// pre-built "who reacted" tooltip text (empty = no tooltip).
    pub fn new(
        emoji: String,
        count: i64,
        viewer_reacted: bool,
        reactors_title: String,
        emojis: &[EmojiRef],
    ) -> Self {
        let display_html = Self::render_emoji(&emoji, emojis);
        Self {
            emoji,
            count,
            viewer_reacted,
            display_html,
            reactors_title,
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

    let placeholders = std::iter::repeat_n("?", deduped.len())
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
    let flat = body.replace(['\r', '\n'], " ");
    let trimmed = flat.trim();
    let mut out = String::new();
    for (count, c) in trimmed.chars().enumerate() {
        if count >= QUOTE_EXCERPT_MAX_CHARS {
            out.push('\u{2026}');
            break;
        }
        out.push(c);
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
    fn to_iso_utc_converts_sqlite_datetime() {
        assert_eq!(to_iso_utc("2026-06-15 14:23:01"), "2026-06-15T14:23:01Z");
    }

    #[test]
    fn system_inline_parses_transcript_link() {
        let out = render_system_inline("[\u{1F4DD} Call transcript saved](/transcripts/47)");
        assert_eq!(
            out,
            "<a href=\"/transcripts/47\" class=\"lc-sysevent-link\">\u{1F4DD} Call transcript saved</a>"
        );
    }

    #[test]
    fn system_inline_plain_text_is_escaped() {
        assert_eq!(render_system_inline("started a call."), "started a call.");
        // No anchor, and any markup is escaped (server-authored, but defensive).
        assert_eq!(render_system_inline("a <b> & c"), "a &lt;b&gt; &amp; c");
    }

    #[test]
    fn system_inline_rejects_external_link() {
        // Only internal `/...` destinations become anchors; a scheme/host link
        // is left as escaped literal text.
        let out = render_system_inline("[x](https://evil.test)");
        assert!(!out.contains("<a "), "external link must not anchor: {out}");
        assert!(
            out.contains("&gt;") || out.contains("[x]"),
            "escaped: {out}"
        );
    }

    #[test]
    fn to_iso_utc_passes_through_nonstandard() {
        assert_eq!(to_iso_utc("not-a-date"), "not-a-date");
        // Already-ISO-ish input is left alone rather than mangled.
        assert_eq!(to_iso_utc(""), "");
    }

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
        let out = render_body("hey @alice check https://example.com", &mentions, &[], &[]);
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
        let out = render_body("ping @nobody", &[], &[], &[]);
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
        let out = render_body("hey @here and @channel quick update", &[], &[], &[]);
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
        let out = render_body("attn @Here and @CHANNEL", &[], &[], &[]);
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
        let out = render_body("can we sync @here?", &[], &[], &[]);
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
        let out = render_body("contact foo@here.example.com", &[], &[], &[]);
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
        let out = render_body("ping foo@bar.com", &mentions, &[], &[]);
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
        let out = render_body("hello :party: world", &[], &emojis, &[]);
        assert!(out.contains("src=\"/api/emojis/42\""), "no img: {out}");
        assert!(out.contains("alt=\":party:\""), "no alt: {out}");
        // Surrounding plain text is preserved and escaped.
        assert!(out.contains("hello "), "leading text lost: {out}");
        assert!(out.contains(" world"), "trailing text lost: {out}");
    }

    #[test]
    fn render_body_leaves_unknown_shortcode_as_text() {
        let out = render_body("plain :missing: text", &[], &[], &[]);
        // Unknown shortcode passes through as literal escaped text.
        assert!(out.contains(":missing:"), "shortcode lost: {out}");
    }

    #[test]
    fn render_body_does_not_substitute_emoji_inside_url() {
        let emojis = vec![EmojiRef {
            id: 1,
            shortcode: "foo".into(),
        }];
        let out = render_body("see https://example.com/:foo:.png ok", &[], &emojis, &[]);
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

    // LC-323: #channel link rewrite.
    #[test]
    fn render_body_rewrites_known_channel_to_link() {
        let channels = vec![ChannelRef {
            name: "general".into(),
            room_id: 7,
        }];
        let out = render_body("see #general for news", &[], &[], &channels);
        assert!(
            out.contains("href=\"/room/7\""),
            "channel link missing: {out}"
        );
        assert!(
            out.contains(">#general</a>"),
            "channel label missing: {out}"
        );
    }

    #[test]
    fn render_body_channel_match_is_case_insensitive() {
        let channels = vec![ChannelRef {
            name: "General".into(),
            room_id: 9,
        }];
        let out = render_body("ping #general", &[], &[], &channels);
        assert!(
            out.contains("href=\"/room/9\""),
            "case-insensitive match failed: {out}"
        );
        // The canonical room name is rendered, not the typed casing.
        assert!(out.contains(">#General</a>"), "canonical name lost: {out}");
    }

    #[test]
    fn render_body_leaves_unknown_channel_as_text() {
        let channels = vec![ChannelRef {
            name: "general".into(),
            room_id: 7,
        }];
        let out = render_body("nothing at #random here", &[], &[], &channels);
        assert!(
            !out.contains("href=\"/room/"),
            "unknown channel linked: {out}"
        );
        assert!(out.contains("#random"), "unknown token lost: {out}");
    }

    #[test]
    fn render_body_no_channels_leaves_hash_literal() {
        let out = render_body("a #general b", &[], &[], &[]);
        assert!(
            !out.contains("href=\"/room/"),
            "linked with empty channel set: {out}"
        );
        assert!(out.contains("#general"), "literal hash token lost: {out}");
    }

    #[test]
    fn render_body_channel_not_matched_mid_word() {
        // Boundary-anchored: `#general` not at a word boundary must not link.
        let channels = vec![ChannelRef {
            name: "general".into(),
            room_id: 7,
        }];
        let out = render_body("foo#general", &[], &[], &channels);
        assert!(!out.contains("href=\"/room/7\""), "mid-word linked: {out}");
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
    /// LC-576: whether the VIEWER has starred this room, for the header's
    /// favorite star. The sidebar rows take `is_starred` as a per-section
    /// literal, so this is the only per-room source of truth on the page.
    pub is_starred: bool,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
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
    /// LC-84: viewer is allowed to grant/revoke per-room role overrides.
    /// Drives the "Moderators" link in the room header.
    pub can_manage_overrides: bool,
    /// LC-85: viewer is allowed to post in this room given the room's
    /// `posting_allowed_for` policy and their effective role. Drives the
    /// compose-box render: when false the composer is replaced with a
    /// read-only notice.
    pub can_post: bool,
    /// LC-480: raw posting policy (`all` / `moderators_only` / `admins_only`).
    /// Drives the announcement banner above the timeline and the localized
    /// read-only notice when `can_post` is false. The template maps the value
    /// to the right `|t` string so the copy is translatable.
    pub posting_policy: &'a str,
    /// LC-64: the user's persisted draft body for this room, if any.
    /// Empty string means "no draft" (whether absent, stale, or just
    /// purged by the 60-day lazy-cleanup rule). The composer template
    /// inlines it as the textarea's initial inner content.
    pub initial_draft: &'a str,
    /// LC-484: gates the header "Catch me up" action (operator LLM configured).
    pub llm_available: bool,
    /// LC-549: an operator embeddings endpoint is configured; drives
    /// `data-lc-embeddings` for the "Find related" and semantic-search gates.
    pub embeddings_available: bool,
    /// LC-506: LLM unconfigured but the viewer is a site admin - show the AI
    /// actions disabled with a "set LETS_CHAT_LLM_URL" hint (discoverability).
    pub llm_teaser: bool,
    /// LC-500: configured max upload size (bytes), surfaced to the composer so
    /// client-side validation matches the server cap.
    pub max_upload_bytes: i64,
    /// LC-488: the GIF picker is configured (LETS_CHAT_GIPHY_API_KEY set).
    /// Drives the composer GIF button.
    pub gif_available: bool,
    /// LC-511: Giphy unconfigured but the viewer is a site admin - show the GIF
    /// button disabled with a "set LETS_CHAT_GIPHY_API_KEY" hint
    /// (discoverability), mirroring the LC-506 AI teaser.
    pub gif_teaser: bool,
    /// LC-489: pre-rendered "Seen by" avatar bar (`partials/room_seen_bar.html`),
    /// empty string when the room is not eligible. Threaded as HTML like
    /// `pinned_strip_html` so the page handler owns the per-viewer query.
    pub room_seen_bar_html: String,
    /// LC-493: huddle support. `huddle_enabled` is true for non-DM rooms (the
    /// ad-hoc audio mesh attaches to the text room); `ice_servers` is the JSON
    /// ICE config for the WebRTC mesh; `huddle_participants` seeds the bar with
    /// anyone already on the line so a fresh page load shows the active huddle.
    pub huddle_enabled: bool,
    pub ice_servers: &'a str,
    pub huddle_participants: Vec<crate::views::voice::VoiceParticipant>,
    /// LC-494: stage mode (large-audience audio control plane). `stage_enabled`
    /// gates the panel; `stage_panel_html` is the pre-rendered roster (empty
    /// string when off), threaded as HTML like the seen bar.
    pub stage_enabled: bool,
    pub stage_panel_html: String,
    /// LC-568: number of rows in `room_members` for this room, surfaced to
    /// the Details panel's "Members" row.
    pub member_count: usize,
    /// LC-553: up to a few member user-ids that seed the header avatar stack
    /// (rendered as /avatars/{id}); `member_count - header_members.len()` is the
    /// "+N" overflow. For public rooms these are enclave members, since a public
    /// room's access derives from enclave membership, not an explicit roster.
    pub header_members: Vec<String>,
    /// LC-568: whether the room has at least one pinned message, surfaced
    /// to the Details panel's "Pinned" row. Computed independently of
    /// `pinned_strip_html` (that fragment always renders a wrapper div even
    /// with zero pins, so it can't be used as an emptiness signal).
    pub has_pinned: bool,
}

impl RoomPage<'_> {
    /// LC-332: message-length cap surfaced to `room/composer.html` as
    /// `data-lc-maxchars` for the character counter. Single source of truth.
    pub fn max_message_chars(&self) -> usize {
        crate::routes::room::MAX_MESSAGE_CHARS
    }
}

#[derive(Template)]
#[template(path = "room/composer.html")]
pub struct ComposerFragment<'a> {
    pub room: &'a Room,
    /// LC-64: same field as on `RoomPage` / `DmPage`. Standalone
    /// composer re-renders (slash command result, link-block result,
    /// post-message success) pass `""` because those paths run AFTER
    /// the action and the user's typed text is either committed
    /// (send / slash) or rejected (link-block); no in-progress draft
    /// to restore from the server here. The next keystroke will
    /// re-create the draft if non-empty.
    pub initial_draft: &'a str,
    /// LC-500: see `RoomPage::max_upload_bytes`. Required for the shared
    /// composer template to compile against this struct.
    pub max_upload_bytes: i64,
    /// LC-488: see `RoomPage::gif_available`. Required for the shared composer
    /// template to compile against this struct.
    pub gif_available: bool,
    /// LC-511: see `RoomPage::gif_teaser`. Required for the shared composer
    /// template to compile against this struct.
    pub gif_teaser: bool,
}

impl ComposerFragment<'_> {
    /// LC-332: see `RoomPage::max_message_chars`. The standalone composer
    /// re-render path needs the same `data-lc-maxchars` value.
    pub fn max_message_chars(&self) -> usize {
        crate::routes::room::MAX_MESSAGE_CHARS
    }
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

/// LC-489: one member in a group room's "Seen by" avatar stack. Avatars resolve
/// via `/avatars/{user_id}` (the route serves a generated fallback), so only the
/// id + display label are needed.
pub struct SeenAvatar {
    pub user_id: String,
    pub label: String,
}

/// LC-489: the "Seen by" avatar stack pinned below a small group room's message
/// list (`#lc-seen-{room_id}`). Shared by the page render and the live OOB swap;
/// an empty `members` renders an empty bar (so it clears live).
#[derive(Template)]
#[template(path = "partials/room_seen_bar.html")]
pub struct RoomSeenBar {
    pub room_id: i64,
    pub members: Vec<SeenAvatar>,
    pub oob: bool,
}

#[derive(Template)]
#[template(path = "partials/reaction_bar.html")]
pub struct ReactionBarFragment<'a> {
    pub message_id: i64,
    pub reactions: &'a [ReactionView],
}

/// LC-490: per-viewer acknowledgement state for a message that requires it.
/// `None` on `MessageView` means the message does not require acknowledgement
/// (the common case); `Some(_)` renders the ack bar.
#[derive(Debug, Clone)]
pub struct AckState {
    /// How many users have acknowledged.
    pub count: i64,
    /// Whether the viewing user has acknowledged.
    pub acked_by_me: bool,
    /// Comma-joined acker display names for the roster tooltip (empty when none).
    pub ackers_title: String,
}

/// LC-490: initial render of the ack bar, included by `room/message.html` and
/// returned by the acknowledge handler for the acting tab.
#[derive(Template)]
#[template(path = "partials/ack_bar.html")]
pub struct AckBarFragment {
    pub message_id: i64,
    pub ack: AckState,
}

/// LC-66: OOB re-render of a poll block after a vote or a close. Swaps the
/// innerHTML of `#poll-{message_id}` for every connected room member.
#[derive(Template)]
#[template(path = "ws/poll_update.html")]
pub struct PollUpdateFragment<'a> {
    pub poll: &'a PollView,
}

/// LC-66: the poll block on its own (no OOB wrapper), returned to the voter
/// as the direct response to `POST /poll/{id}/vote` (hx-swap=innerHTML into
/// `#poll-{id}`).
#[derive(Template)]
#[template(path = "partials/poll_block.html")]
pub struct PollBlockFragment<'a> {
    pub poll: &'a PollView,
}

/// LC-527: OOB update of a follow-up block, pushed to the room over the WS
/// after an item is toggled or (un)claimed.
#[derive(Template)]
#[template(path = "ws/followup_update.html")]
pub struct FollowUpUpdateFragment<'a> {
    pub follow_up: &'a FollowUpView,
}

/// LC-527: the follow-up block on its own (no OOB wrapper), returned to the
/// acting user as the direct response to a toggle / claim (hx-swap=innerHTML
/// into `#followup-{id}`).
#[derive(Template)]
#[template(path = "partials/followup_block.html")]
pub struct FollowUpBlockFragment<'a> {
    pub follow_up: &'a FollowUpView,
}

/// Right-side thread drawer. Replaces `#thread-panel` outerHTML when opened.
#[derive(Template)]
#[template(path = "room/thread_panel.html")]
pub struct ThreadPanelFragment<'a> {
    pub room: &'a Room,
    pub parent: &'a MessageView,
    pub replies: &'a [MessageView],
    /// LC-310: whether the viewer follows this thread (drives the toggle).
    pub is_following: bool,
    /// LC-546: whether the viewer has muted this thread (drives the mute toggle).
    pub is_muted: bool,
    /// LC-484: gates the "Summarize" action (operator LLM configured).
    pub llm_available: bool,
    /// LC-549: an operator embeddings endpoint is configured; drives
    /// `data-lc-embeddings` for the "Find related" and semantic-search gates.
    pub embeddings_available: bool,
}

/// LC-310: the thread Follow/Following toggle button, included by the thread
/// panel and re-rendered by the follow/unfollow endpoints.
#[derive(Template)]
#[template(path = "partials/thread_follow_button.html")]
pub struct ThreadFollowFragment {
    pub room_id: i64,
    pub parent_id: i64,
    pub following: bool,
}

/// LC-546: the thread Mute/Muted toggle button, included by the thread panel
/// and re-rendered by the mute/unmute endpoints.
#[derive(Template)]
#[template(path = "partials/thread_mute_button.html")]
pub struct ThreadMuteFragment {
    pub room_id: i64,
    pub parent_id: i64,
    pub muted: bool,
}

/// Empty thread panel container, used to close the drawer.
#[derive(Template)]
#[template(path = "room/thread_panel_closed.html")]
pub struct ThreadPanelClosedFragment;

/// One entry in the edit-history drawer. `body_html` is pre-rendered by the
/// markdown pipeline so the template emits it with `|safe`. `label` is
/// pre-computed in the handler ("Edited <ts>" for prior bodies, "Current"
/// or "Current - last edited <ts>" for the live body) so the template stays
/// branch-free and consistent with the codebase's `{% if %}`-only
/// convention (no `{% match %}` in any template today).
pub struct HistoryEntryView {
    pub body_html: String,
    pub label: String,
}

/// Right-side edit-history drawer. Replaces `#history-panel` outerHTML when
/// opened. Sibling slot to `#thread-panel`; the two drawers do not share
/// state and can coexist.
#[derive(Template)]
#[template(path = "room/history_panel.html")]
pub struct HistoryPanelFragment<'a> {
    pub message_id: i64,
    pub entries: &'a [HistoryEntryView],
}

/// Empty history panel container, used to close the drawer.
#[derive(Template)]
#[template(path = "room/history_panel_closed.html")]
pub struct HistoryPanelClosedFragment;

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
