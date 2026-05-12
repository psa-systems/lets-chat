//! Email-digest content primitives (phase 22 tasks 3 + 4).
//!
//! `build_snippet` is the only public entry point in this file today. It
//! takes a raw message body and returns a `(plaintext, html)` pair suitable
//! for inclusion in the digest's two-part `multipart/alternative` body.
//!
//! Future tasks (4, scheduler tick) layer the eligibility query and dispatch
//! loop on top of this module; they reuse `build_snippet` to render each
//! digest item.
//!
//! Design notes:
//! - The digest renders messages MUCH more conservatively than the in-app
//!   message bubble (see `views::room::render_body`). No custom emojis, no
//!   profile-link chips, no avatars: email clients have wildly different
//!   HTML support and the safe set is "escaped text, bold, anchor."
//! - `@username` substrings get `<strong>` treatment in the HTML form
//!   without any resolution lookup. This matches what the author typed
//!   rather than what the mention actually pointed at; for digest copy that
//!   is the right surface (the recipient sees the same prefix they would
//!   in the app's notification toast).
//! - URLs are linkified in the HTML form only. The plaintext form leaves
//!   them as literal text so mail clients with conservative URL detection
//!   still click through them correctly.

/// Render a one-message snippet for inclusion in a digest item.
///
/// Returns `(plain, html)`:
///   * `plain`: trimmed, word-truncated to ~140 chars, ASCII ellipsis when
///     truncated. No escaping.
///   * `html`: same truncation, HTML-escaped, then `@name` substrings
///     wrapped in `<strong>` and URLs wrapped in anchor tags.
///
/// The 140-char cap is the same shape used by Push payloads and the in-app
/// snippet (phase 16). Word-boundary truncation avoids "...take a l" mid-word
/// breaks; the loop accumulates whitespace-separated chunks while their
/// combined length plus the next chunk fits.
pub fn build_snippet(body: &str) -> (String, String) {
    const MAX_CHARS: usize = 140;
    let trimmed = body.trim();
    let (truncated, truncated_did) = word_truncate(trimmed, MAX_CHARS);
    let plain = if truncated_did {
        format!("{truncated}...")
    } else {
        truncated.to_string()
    };
    let html_inner = render_html_snippet(truncated);
    let html = if truncated_did {
        format!("{html_inner}...")
    } else {
        html_inner
    };
    (plain, html)
}

/// Truncate `s` at a whitespace boundary so the result fits within
/// `max_chars` UTF-8 bytes (with room for a trailing "..." that the
/// caller appends). Returns `(slice, was_truncated)` where `slice` is a
/// sub-slice of the input, never split mid-grapheme because the cut is
/// always at an ASCII whitespace byte.
fn word_truncate(s: &str, max_chars: usize) -> (&str, bool) {
    if s.len() <= max_chars {
        return (s, false);
    }
    // Walk byte indices at whitespace boundaries from the front; keep the
    // last boundary that still fits in `max_chars`.
    let mut last_fit = 0usize;
    let mut chunks = s.split_inclusive(char::is_whitespace);
    let mut acc = 0usize;
    for chunk in &mut chunks {
        let next = acc + chunk.len();
        if next > max_chars {
            break;
        }
        acc = next;
        last_fit = acc;
    }
    // `last_fit` may have a trailing whitespace from `split_inclusive`;
    // trim it off so the rendered "...":
    //   "Hello world ..." -> "Hello world..."
    let cut = s[..last_fit].trim_end();
    if cut.is_empty() {
        // A single 200-char word with no whitespace cannot be word-broken.
        // Fall back to a hard cut at a char boundary <= max_chars.
        let mut i = max_chars;
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        (&s[..i], true)
    } else {
        (cut, true)
    }
}

fn render_html_snippet(s: &str) -> String {
    use std::sync::OnceLock;
    static MENTION_RE: OnceLock<regex::Regex> = OnceLock::new();
    // Same shape as `db::mentions::TOKEN_PATTERN`: leading whitespace OR
    // start-of-string, then `@` + 1-32 chars of `[A-Za-z0-9_-]`. Captures
    // the username so we can wrap it in <strong> without including the
    // leading whitespace inside the tag.
    let mention_re = MENTION_RE
        .get_or_init(|| regex::Regex::new(r"(?:^|\s)@([A-Za-z0-9_-]{1,32})").expect("regex"));

    // Pass 1: linkify URL runs. Operates on the raw (pre-escape) text so
    // the `LinkFinder` sees the URLs as the user typed them.
    let finder = linkify::LinkFinder::new();
    let mut out = String::with_capacity(s.len() + 32);
    let mut cursor = 0usize;
    for link in finder.links(s) {
        if !matches!(link.kind(), linkify::LinkKind::Url) {
            continue;
        }
        let start = link.start();
        let end = link.end();
        if start > cursor {
            out.push_str(&apply_mention_bold(
                &html_escape(&s[cursor..start]),
                mention_re,
            ));
        }
        let url = link.as_str();
        out.push_str("<a href=\"");
        out.push_str(&html_escape(url));
        out.push_str("\">");
        out.push_str(&html_escape(url));
        out.push_str("</a>");
        cursor = end;
    }
    if cursor < s.len() {
        out.push_str(&apply_mention_bold(&html_escape(&s[cursor..]), mention_re));
    }
    out
}

/// Wrap each `@name` substring in `<strong>@name</strong>`. Operates on
/// already-HTML-escaped text so the regex matches the literal `@` followed
/// by the entity-free username (escaping does not affect `[A-Za-z0-9_-]`).
fn apply_mention_bold(escaped: &str, re: &regex::Regex) -> String {
    let mut out = String::with_capacity(escaped.len() + 16);
    let mut cursor = 0usize;
    for cap in re.captures_iter(escaped) {
        let whole = cap.get(0).unwrap();
        let name = cap.get(1).unwrap();
        // The leading boundary (whitespace or BOS) lives at whole.start();
        // the `@` starts one byte before name.start(). Copy everything up
        // to the `@` verbatim so the boundary character is preserved.
        let at_pos = name.start() - 1;
        if at_pos > cursor {
            out.push_str(&escaped[cursor..at_pos]);
        }
        out.push_str("<strong>@");
        out.push_str(name.as_str());
        out.push_str("</strong>");
        cursor = whole.end();
    }
    if cursor < escaped.len() {
        out.push_str(&escaped[cursor..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Tick (task 4): orchestrate eligibility + per-user fetch + send.
// ---------------------------------------------------------------------------

use std::collections::{BTreeMap, HashMap, HashSet};

use sqlx::{Row, SqlitePool};

use crate::error::AppError;
use crate::mail::Mailer;
use crate::state::AppState;
use crate::views::email_digest::{
    DigestDmSection, DigestHtml, DigestItem, DigestRoomSection, DigestText,
};
use askama::Template;

/// Per-tick configuration. Held as a struct so the scheduler in
/// `main.rs` can pin the constants in one place without sprinkling
/// magic numbers through this module.
#[derive(Debug, Clone, Copy)]
pub struct DigestConfig {
    pub quiet_period_secs: i64,
    pub max_items: usize,
    pub time_window_days: i64,
}

impl Default for DigestConfig {
    fn default() -> Self {
        Self {
            quiet_period_secs: 3600,
            max_items: 50,
            time_window_days: 7,
        }
    }
}

/// One pass of the digest scheduler.
///
/// Short-circuits when `state.mailer` is `None` (the operator has not
/// configured SMTP via env vars). Otherwise loads candidate users,
/// fetches their missed activity, renders multipart bodies, and
/// dispatches one email per candidate through the shared `Mailer`.
/// The "one digest per offline session" predicate lives in
/// `db::auth::find_digest_candidates`; this function only needs to mark
/// `last_digest_sent_at` on success to make the predicate self-reset on
/// the next activity bump.
///
/// Errors are logged at warn and skipped per-user. A single bad
/// recipient must not stop the rest of the tick from processing.
pub async fn run_tick(state: &AppState, cfg: DigestConfig) -> Result<(), AppError> {
    let Some(mailer) = state.mailer.as_ref() else {
        tracing::debug!("digest tick: mailer is None, skipping");
        return Ok(());
    };

    let server_url = state.base_url.as_str();
    let candidates =
        crate::db::auth::find_digest_candidates(&state.auth, cfg.quiet_period_secs).await?;
    tracing::debug!(count = candidates.len(), "digest tick: candidates");

    for candidate in candidates {
        if let Err(e) = send_one_digest(state, mailer, &candidate, server_url, &cfg).await {
            tracing::warn!(
                error = %e,
                user_id = %candidate.id,
                "digest send failed for user; will retry next tick"
            );
        }
    }
    Ok(())
}

/// Build and send a digest for a single candidate. Returns `Ok(())` if
/// the user had nothing missed (no-op) OR if the send succeeded. Returns
/// `Err` only on a real failure (DB error, mail transport error). The
/// caller logs and continues to the next candidate.
async fn send_one_digest(
    state: &AppState,
    mailer: &Mailer,
    candidate: &crate::db::auth::DigestCandidate,
    server_url: &str,
    cfg: &DigestConfig,
) -> Result<(), AppError> {
    let mentions = load_missed_mentions(
        &state.chat,
        &candidate.id,
        &candidate.activity_floor,
        cfg.time_window_days,
    )
    .await?;
    let dms = load_missed_dms(
        &state.chat,
        &candidate.id,
        &candidate.activity_floor,
        cfg.time_window_days,
    )
    .await?;

    if mentions.is_empty() && dms.is_empty() {
        // Either the activity got muted/read between candidate-selection
        // and this point, or the candidate had no real misses. Either way
        // do not mark `last_digest_sent_at`: the eligibility predicate
        // already re-checks the floor on each tick.
        return Ok(());
    }

    // Bulk-resolve every author + peer id in one auth-pool round trip.
    let mut id_set: HashSet<&str> = HashSet::new();
    for m in &mentions {
        id_set.insert(m.author_id.as_str());
    }
    for d in &dms {
        id_set.insert(d.author_id.as_str());
        id_set.insert(d.peer_id.as_str());
    }
    let id_vec: Vec<&str> = id_set.into_iter().collect();
    let name_map = crate::db::auth::display_names_for_ids(&state.auth, &id_vec).await?;

    // Build sections, applying the per-digest item cap. DMs come first
    // (higher signal than channel mentions); within each section, the
    // SQL already ordered by created_at, so we preserve oldest-first
    // ordering during grouping.
    let mut budget = cfg.max_items;
    let mut overflow = 0usize;

    let (dm_sections, dm_overflow) = build_dm_sections(&dms, &name_map, server_url, &mut budget);
    overflow += dm_overflow;
    let (room_sections, room_overflow) =
        build_room_sections(&mentions, &name_map, server_url, &mut budget);
    overflow += room_overflow;

    let html_body = DigestHtml {
        server_url,
        dm_sections: &dm_sections,
        room_sections: &room_sections,
        overflow_count: overflow,
    }
    .render()
    .map_err(|e| AppError::Internal(format!("digest html render: {e}")))?;
    let text_body = DigestText {
        server_url,
        dm_sections: &dm_sections,
        room_sections: &room_sections,
        overflow_count: overflow,
    }
    .render()
    .map_err(|e| AppError::Internal(format!("digest text render: {e}")))?;

    let mention_count: usize = room_sections.iter().map(|s| s.items.len()).sum();
    let dm_count: usize = dm_sections.iter().map(|s| s.items.len()).sum();
    let subject = build_subject(mention_count, dm_count);

    mailer
        .send_multipart(&candidate.email, &subject, &text_body, &html_body)
        .await
        .map_err(|e| AppError::Internal(format!("smtp send: {e}")))?;
    crate::db::auth::set_last_digest_sent_at(&state.auth, &candidate.id).await?;
    Ok(())
}

fn build_subject(mention_count: usize, dm_count: usize) -> String {
    let mentions_clause = match mention_count {
        0 => None,
        1 => Some("1 new mention".to_string()),
        n => Some(format!("{n} new mentions")),
    };
    let dms_clause = match dm_count {
        0 => None,
        1 => Some("1 direct message".to_string()),
        n => Some(format!("{n} direct messages")),
    };
    let body = match (mentions_clause, dms_clause) {
        (Some(m), Some(d)) => format!("{m} and {d}"),
        (Some(m), None) => m,
        (None, Some(d)) => d,
        // Caller already short-circuits when both lists are empty, so
        // this branch is unreachable in practice; fall back to a generic
        // subject rather than panic.
        (None, None) => "new activity".to_string(),
    };
    format!("[lets-chat] {body}")
}

/// Group raw mention rows by room and apply the `budget` cap. The cap
/// counts items across all sections together; once budget hits 0, the
/// remaining items are added to `overflow` so the digest can render
/// "...and N more."
fn build_room_sections(
    mentions: &[MissedMention],
    name_map: &HashMap<String, (String, Option<String>)>,
    server_url: &str,
    budget: &mut usize,
) -> (Vec<DigestRoomSection>, usize) {
    let mut by_room: BTreeMap<i64, DigestRoomSection> = BTreeMap::new();
    let mut overflow = 0usize;
    for m in mentions {
        if *budget == 0 {
            overflow += 1;
            continue;
        }
        let section = by_room
            .entry(m.room_id)
            .or_insert_with(|| DigestRoomSection {
                room_name: m.room_name.clone(),
                room_id: m.room_id,
                items: Vec::new(),
            });
        section.items.push(build_item(
            m.message_id,
            &m.author_id,
            &m.body,
            &m.created_at,
            name_map,
            deep_link_room(server_url, m.room_id, m.message_id),
        ));
        *budget -= 1;
    }
    (by_room.into_values().collect(), overflow)
}

fn build_dm_sections(
    dms: &[MissedDm],
    name_map: &HashMap<String, (String, Option<String>)>,
    server_url: &str,
    budget: &mut usize,
) -> (Vec<DigestDmSection>, usize) {
    let mut by_peer: BTreeMap<String, DigestDmSection> = BTreeMap::new();
    let mut overflow = 0usize;
    for d in dms {
        if *budget == 0 {
            overflow += 1;
            continue;
        }
        let section = by_peer
            .entry(d.peer_id.clone())
            .or_insert_with(|| DigestDmSection {
                peer_username: name_map
                    .get(&d.peer_id)
                    .map(|(u, _)| u.clone())
                    .unwrap_or_else(|| d.peer_id.clone()),
                peer_id: d.peer_id.clone(),
                items: Vec::new(),
            });
        section.items.push(build_item(
            d.message_id,
            &d.author_id,
            &d.body,
            &d.created_at,
            name_map,
            deep_link_dm(server_url, &d.peer_id, d.message_id),
        ));
        *budget -= 1;
    }
    (by_peer.into_values().collect(), overflow)
}

fn build_item(
    message_id: i64,
    author_id: &str,
    body: &str,
    created_at: &str,
    name_map: &HashMap<String, (String, Option<String>)>,
    deep_link: String,
) -> DigestItem {
    let author = name_map
        .get(author_id)
        .map(|(u, _)| u.clone())
        .unwrap_or_else(|| author_id.to_string());
    let (snippet_plain, snippet_html) = build_snippet(body);
    DigestItem {
        message_id,
        author,
        created_at: format_timestamp(created_at),
        snippet_plain,
        snippet_html,
        deep_link,
    }
}

/// Render an ISO 8601 / sqlite-`datetime('now')` string as a short
/// human-friendly "Mon 14:23" form. Falls back to the raw input on parse
/// error so the digest never panics; mail clients display whatever made
/// it through.
fn format_timestamp(ts: &str) -> String {
    use chrono::{DateTime, NaiveDateTime, Utc};
    // SQLite datetime('now') emits "YYYY-MM-DD HH:MM:SS" without timezone.
    // chrono parses that as NaiveDateTime. Treat it as UTC for the
    // weekday/HH:MM rendering; we are not trying to localise per
    // recipient (yet) - that is a follow-up if it ever matters.
    if let Ok(naive) = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        let dt: DateTime<Utc> = naive.and_utc();
        return dt.format("%a %H:%M").to_string();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return dt.format("%a %H:%M").to_string();
    }
    ts.to_string()
}

fn deep_link_room(server_url: &str, room_id: i64, message_id: i64) -> String {
    if server_url.is_empty() {
        return String::new();
    }
    format!("{server_url}/room/{room_id}#m{message_id}")
}

fn deep_link_dm(server_url: &str, peer_id: &str, message_id: i64) -> String {
    if server_url.is_empty() {
        return String::new();
    }
    format!("{server_url}/dm/{peer_id}#m{message_id}")
}

#[derive(Debug, Clone)]
struct MissedMention {
    room_id: i64,
    room_name: String,
    message_id: i64,
    author_id: String,
    body: String,
    created_at: String,
}

#[derive(Debug, Clone)]
struct MissedDm {
    peer_id: String,
    message_id: i64,
    author_id: String,
    body: String,
    created_at: String,
}

/// Load unread room mentions in the digest window. Mute predicate:
/// absence of a `room_notification_settings` row OR a row with
/// `mute_mode <> 'all'`. `'except_mentions'` is allowed through because
/// it explicitly lets @mentions land while suppressing ambient unread.
async fn load_missed_mentions(
    chat_pool: &SqlitePool,
    user_id: &str,
    activity_floor: &str,
    time_window_days: i64,
) -> Result<Vec<MissedMention>, AppError> {
    let window_modifier = format!("-{time_window_days} days");
    let rows = sqlx::query(
        "SELECT m.message_id, m.room_id, msg.body, msg.user_id AS author_id, \
                msg.created_at, r.name AS room_name \
           FROM mentions m \
           JOIN messages msg ON msg.id = m.message_id \
           JOIN rooms    r   ON r.id = m.room_id \
           LEFT JOIN room_notification_settings rns \
                  ON rns.user_id = m.mentioned_user_id AND rns.room_id = m.room_id \
          WHERE m.mentioned_user_id = ? \
            AND m.read_at IS NULL \
            AND msg.deleted_at IS NULL \
            AND msg.created_at > ? \
            AND msg.created_at > datetime('now', ?) \
            AND (rns.mute_mode IS NULL OR rns.mute_mode <> 'all') \
          ORDER BY r.id, msg.created_at",
    )
    .bind(user_id)
    .bind(activity_floor)
    .bind(window_modifier)
    .fetch_all(chat_pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MissedMention {
            room_id: r.get("room_id"),
            room_name: r.get("room_name"),
            message_id: r.get("message_id"),
            author_id: r.get("author_id"),
            body: r.get("body"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Load unread DM messages from `dm_read_state` watermark. Mute uses the
/// shared `room_notification_settings` table - DM mute writes `'all'` to
/// the same table keyed by the DM's room id (see `db::notifications::set_dm_mute`).
async fn load_missed_dms(
    chat_pool: &SqlitePool,
    user_id: &str,
    activity_floor: &str,
    time_window_days: i64,
) -> Result<Vec<MissedDm>, AppError> {
    let window_modifier = format!("-{time_window_days} days");
    let rows = sqlx::query(
        "SELECT msg.id AS message_id, \
                peer.user_id AS peer_id, \
                msg.user_id AS author_id, msg.body, msg.created_at \
           FROM messages msg \
           JOIN rooms r ON r.id = msg.room_id AND r.room_type = 'dm' \
           JOIN room_members rm_self \
                  ON rm_self.room_id = r.id AND rm_self.user_id = ? \
           JOIN room_members peer \
                  ON peer.room_id = r.id AND peer.user_id <> ? \
           LEFT JOIN dm_read_state s \
                  ON s.user_id = ? AND s.room_id = r.id \
           LEFT JOIN room_notification_settings rns \
                  ON rns.user_id = ? AND rns.room_id = r.id \
          WHERE msg.user_id <> ? \
            AND msg.deleted_at IS NULL \
            AND msg.id > COALESCE(s.last_read_message_id, 0) \
            AND msg.created_at > ? \
            AND msg.created_at > datetime('now', ?) \
            AND (rns.mute_mode IS NULL OR rns.mute_mode <> 'all') \
          ORDER BY peer.user_id, msg.created_at",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(activity_floor)
    .bind(window_modifier)
    .fetch_all(chat_pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MissedDm {
            peer_id: r.get("peer_id"),
            message_id: r.get("message_id"),
            author_id: r.get("author_id"),
            body: r.get("body"),
            created_at: r.get("created_at"),
        })
        .collect())
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_round_trips_unchanged() {
        let (plain, html) = build_snippet("Hello world");
        assert_eq!(plain, "Hello world");
        assert_eq!(html, "Hello world");
    }

    #[test]
    fn trim_strips_leading_and_trailing_whitespace() {
        let (plain, _) = build_snippet("   spaced out   ");
        assert_eq!(plain, "spaced out");
    }

    #[test]
    fn long_string_word_truncates_with_ellipsis() {
        // Build a body comfortably over the 140-char cap so truncation
        // must engage. Each repeat of the chunk is 23 chars (incl. trailing
        // space); 10 repeats = 230 chars.
        let body = "alpha beta gamma delta ".repeat(10);
        assert!(body.len() > 140, "test setup: body must exceed cap");
        let (plain, html) = build_snippet(&body);
        assert!(
            plain.ends_with("..."),
            "plaintext should end with ellipsis; got {plain:?}"
        );
        assert!(
            html.ends_with("..."),
            "html should end with ellipsis; got {html:?}"
        );
        // Word-truncate: the last word before "..." must be a complete
        // token from the input vocabulary, never a partial slice.
        let plain_no_dots = plain.trim_end_matches('.');
        let last_word = plain_no_dots.split_whitespace().next_back().unwrap();
        assert!(
            ["alpha", "beta", "gamma", "delta"].contains(&last_word),
            "last word {last_word:?} should be one of alpha/beta/gamma/delta"
        );
        // Length cap: the truncated content (before "...") must fit
        // within MAX_CHARS.
        assert!(
            plain_no_dots.len() <= 140,
            "plain still over cap: {} chars",
            plain_no_dots.len()
        );
    }

    #[test]
    fn html_escapes_special_characters_but_plain_leaves_them() {
        let body = r#"a & b <c> "d" 'e'"#;
        let (plain, html) = build_snippet(body);
        assert_eq!(plain, r#"a & b <c> "d" 'e'"#);
        assert!(html.contains("&amp;"));
        assert!(html.contains("&lt;c&gt;"));
        assert!(html.contains("&quot;d&quot;"));
        assert!(html.contains("&#39;e&#39;"));
        // The literal chars must NOT appear unescaped.
        assert!(!html.contains(" & "), "html still has unescaped ampersand");
        assert!(
            !html.contains("<c>"),
            "html still has unescaped angle bracket"
        );
    }

    #[test]
    fn mentions_bolded_in_html_only() {
        let body = "Hey @alice and @bob_42-x can you take a look?";
        let (plain, html) = build_snippet(body);
        assert_eq!(plain, "Hey @alice and @bob_42-x can you take a look?");
        assert!(
            html.contains("<strong>@alice</strong>"),
            "expected @alice bolded; got {html:?}"
        );
        assert!(
            html.contains("<strong>@bob_42-x</strong>"),
            "expected @bob_42-x bolded; got {html:?}"
        );
    }

    #[test]
    fn email_address_is_not_treated_as_mention() {
        // Same boundary rule as the mention parser: `@` must be preceded
        // by whitespace or start-of-string, so `foo@bar.com` should NOT
        // produce a `<strong>@bar</strong>` chip.
        let body = "Mail me at foo@bar.com please.";
        let (_, html) = build_snippet(body);
        assert!(
            !html.contains("<strong>"),
            "email-style @ should not produce a mention chip; got {html:?}"
        );
    }

    #[test]
    fn urls_linkified_in_html_only() {
        let body = "See https://example.com/path?q=1 for details";
        let (plain, html) = build_snippet(body);
        assert_eq!(plain, "See https://example.com/path?q=1 for details");
        assert!(
            html.contains(r#"<a href="https://example.com/path?q=1">"#),
            "expected url anchor; got {html:?}"
        );
    }

    #[test]
    fn mention_then_url_both_render() {
        let body = "Hey @alice look at https://example.com";
        let (_, html) = build_snippet(body);
        assert!(html.contains("<strong>@alice</strong>"));
        assert!(html.contains(r#"<a href="https://example.com">"#));
    }

    #[test]
    fn single_long_word_falls_back_to_hard_cut() {
        // A single 200-char "word" has no whitespace to break at. The
        // helper should still cap length rather than emit unbounded text.
        let body = "x".repeat(200);
        let (plain, _) = build_snippet(&body);
        assert!(plain.ends_with("..."));
        // Body of 'x' chars plus 3 dots; the cut respects char boundaries
        // and stays at-or-under the cap.
        assert!(plain.len() <= 140 + 3, "got len {}: {plain:?}", plain.len());
    }

    #[test]
    fn no_break_inside_url_html_treats_url_as_atomic_segment() {
        // Even with mention bolding active, the URL anchor must stay
        // intact (no <strong> wrapping inside the href text).
        let body = "@alice https://example.com/about";
        let (_, html) = build_snippet(body);
        assert!(html.contains(r#"<a href="https://example.com/about">"#));
        // The URL contents inside the anchor should NOT be mention-bolded:
        // it has no @ but if a future URL contained @ we would rely on this
        // separation. Sanity check: only one <strong> in the output (the
        // leading @alice), not a stray one inside the URL.
        assert_eq!(
            html.matches("<strong>").count(),
            1,
            "exactly one mention chip expected: {html:?}"
        );
    }
}
