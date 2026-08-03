//! LC-548: AI suggested replies. A "Suggest reply" action on a received message
//! sends that message plus a short window of prior turns to the operator LLM and
//! returns 2-3 tappable draft chips. A chip drops into the composer on tap; it is
//! never auto-sent. Second action mode of the composer-LLM path (LC-532): the
//! result renders into the same `#composer-ai-panel` slot and reuses the
//! `data-lc-ai-apply` / `data-lc-ai-dismiss` handlers in `live.js`.
//!
//! Same trust posture as `routes::compose_assist` and `routes::summary`: gated on
//! an LLM being configured, one `complete()` call, room access enforced. Chat text
//! leaves the device only on an explicit tap and only when an LLM is configured.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::db::chat::RawMessage;
use crate::error::AppError;
#[allow(unused_imports)]
use crate::i18n::filters;
use crate::state::AppState;
use crate::views::{html, Html};
use askama::Template; // LC-188: in-scope for the |t template filter.

/// How many recent top-level turns to feed the model as context (including the
/// received message). Small: a couple of turns is enough to draft a reply, and a
/// tight window keeps cost/latency low on a metered or CPU-bound engine.
const CONTEXT_TURNS: i64 = 10;
/// Cap on the assembled prompt text (mirrors the summary handler's guard).
const MAX_PROMPT_CHARS: usize = 8_000;
/// Most reply chips to show. The model is asked for 2-3; this bounds a chatty one.
const MAX_SUGGESTIONS: usize = 3;
/// Longest single suggestion kept (a runaway "reply" is a rewrite, not a chip).
const MAX_SUGGESTION_CHARS: usize = 400;

/// Draft-reply system prompt. Asks for a few short candidate replies from the
/// viewer's point of view, one per line, with nothing else - so the newline
/// split in [`parse_suggestions`] yields clean chips.
const SYSTEM_PROMPT: &str = "You are a reply assistant for a chat app. Given the recent \
conversation, draft 2 or 3 short, natural replies the user could send next, written from the \
user's point of view. Match the conversation's language and tone. Keep each reply to one or two \
sentences. Do not number or label them, do not add quotes or any preamble. Reply with ONLY the \
candidate messages, one per line.";

/// `display_name` if non-empty, else `username` (the codebase-wide label rule).
fn label_for(username: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => username.to_string(),
    }
}

/// Turn the model's raw completion into clean chips: one per line, list markers
/// and surrounding quotes stripped, blanks dropped, over-long lines skipped,
/// capped at [`MAX_SUGGESTIONS`]. Pure, so it is unit-tested directly.
fn parse_suggestions(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut s = line.trim();
        // Strip a leading ordered- or bullet-list marker the model may add
        // despite instructions ("1. ", "2) ", "- ", "* ", "• ").
        if let Some(rest) = strip_list_marker(s) {
            s = rest.trim_start();
        }
        // Strip a single pair of surrounding quotes.
        s = s
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .unwrap_or(s)
            .trim();
        if s.is_empty() || s.chars().count() > MAX_SUGGESTION_CHARS {
            continue;
        }
        if !out.iter().any(|e: &String| e == s) {
            out.push(s.to_string());
        }
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

/// If `s` opens with a list marker, return the remainder after it.
fn strip_list_marker(s: &str) -> Option<&str> {
    for m in ["- ", "* ", "• "] {
        if let Some(rest) = s.strip_prefix(m) {
            return Some(rest);
        }
    }
    // "12. " / "3) " ordered marker: a short run of digits then '.'/' )' + space.
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 2 {
        let rest = &s[digits.len()..];
        for m in [". ", ") "] {
            if let Some(r) = rest.strip_prefix(m) {
                return Some(r);
            }
        }
    }
    None
}

/// Build the "Label: body" context block (chronological), skipping system and
/// empty messages, then bound it with the shared LC-630 sliding window so the
/// most recent turns win a fixed budget and per-request cost stays flat.
fn build_context(msgs: &[RawMessage], labels: &HashMap<String, String>) -> String {
    let lines: Vec<String> = msgs
        .iter()
        .filter(|m| !m.is_system && !m.body.trim().is_empty())
        .map(|m| {
            let label = labels
                .get(&m.user_id)
                .cloned()
                .unwrap_or_else(|| m.user_id.clone());
            format!("{label}: {}\n", m.body.trim())
        })
        .collect();
    crate::llm::sliding_window(&lines, crate::llm::CONTEXT_MAX_TURNS, MAX_PROMPT_CHARS).concat()
}

/// One reply chip.
struct Suggestion {
    text: String,
}

#[derive(Template)]
#[template(path = "room/suggest_reply_panel.html")]
struct SuggestReplyPanel {
    suggestions: Vec<Suggestion>,
}

/// POST /messages/{message_id}/suggest-reply - draft 2-3 replies to this message
/// using recent room context. Renders into `#composer-ai-panel`; chips are
/// applied into the composer by the shared `live.js` handler.
pub async fn post_suggest_reply(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "reply suggestions are not configured".into(),
        ));
    };

    let message = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, message.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // Window of recent top-level turns ending at (and including) the target.
    // `list_recent_messages` is newest-first and top-level-only; a thread reply
    // won't appear there, so we append it explicitly when missing.
    let recent =
        db::chat::list_recent_messages(&state.chat, message.room_id, CONTEXT_TURNS).await?;
    let mut window: Vec<RawMessage> = recent.into_iter().filter(|m| m.id <= message_id).collect();
    window.reverse(); // chronological for the prompt
    if !window.iter().any(|m| m.id == message_id) {
        window.push(message);
    }

    let mut unique: HashSet<&str> = HashSet::new();
    for m in &window {
        unique.insert(m.user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let resolved = db::auth::display_names_for_ids(&state.auth, &ids).await?;
    let labels: HashMap<String, String> = resolved
        .into_iter()
        .map(|(id, (u, d))| (id, label_for(&u, d.as_deref())))
        .collect();

    let context = build_context(&window, &labels);
    if context.trim().is_empty() {
        return Err(AppError::BadRequest("nothing to reply to".into()));
    }

    let raw = match llm.complete_guarded(SYSTEM_PROMPT, &context).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "suggest-reply failed");
            return Err(AppError::BadRequest("reply suggestions failed".into()));
        }
    };
    let suggestions: Vec<Suggestion> = parse_suggestions(&raw)
        .into_iter()
        .map(|text| Suggestion { text })
        .collect();
    if suggestions.is_empty() {
        return Err(AppError::BadRequest(
            "no reply suggestions were returned".into(),
        ));
    }

    html(&SuggestReplyPanel { suggestions })
}

#[cfg(test)]
mod tests {
    use super::{parse_suggestions, strip_list_marker, MAX_SUGGESTIONS};

    #[test]
    fn strips_ordered_and_bullet_markers() {
        assert_eq!(strip_list_marker("1. hello"), Some("hello"));
        assert_eq!(strip_list_marker("2) hi"), Some("hi"));
        assert_eq!(strip_list_marker("- yo"), Some("yo"));
        assert_eq!(strip_list_marker("* yo"), Some("yo"));
        assert_eq!(strip_list_marker("plain"), None);
        // Not a marker: too many digits / no delimiter.
        assert_eq!(strip_list_marker("1234. x"), None);
        assert_eq!(strip_list_marker("10 apples"), None);
    }

    #[test]
    fn parses_lines_into_clean_chips() {
        let raw = "1. Sounds good, see you then!\n2. \"Thanks for the heads up.\"\n- Can we push it an hour?";
        let got = parse_suggestions(raw);
        assert_eq!(
            got,
            vec![
                "Sounds good, see you then!",
                "Thanks for the heads up.",
                "Can we push it an hour?",
            ]
        );
    }

    #[test]
    fn drops_blanks_and_dedups_and_caps() {
        let raw = "Hi\n\nHi\n   \nOne\nTwo\nThree\nFour";
        let got = parse_suggestions(raw);
        // "Hi" once (dedup), then One/Two -> capped at MAX_SUGGESTIONS total.
        assert_eq!(got.len(), MAX_SUGGESTIONS);
        assert_eq!(got[0], "Hi");
        assert_eq!(got[1], "One");
    }

    #[test]
    fn empty_completion_yields_no_chips() {
        assert!(parse_suggestions("   \n\n").is_empty());
    }
}
