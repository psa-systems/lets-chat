//! LC-668: auto thread titles.
//!
//! Once a thread reaches a few replies and has no title yet, a background task
//! summarizes it (root + replies) into a short label shown as the thread panel
//! heading. Async and tolerant (a short label), private (the operator's own
//! model), and cheap - it reuses the catch-me-up prompt helpers. A no-op without
//! an LLM. Titled once: the stored title is the "already done" marker.

use crate::error::AppError;
use crate::state::AppState;
use crate::{db, routes::summary};

/// Generate a title once a thread reaches this many replies.
const MIN_REPLIES: i64 = 3;
/// Cap on the stored title length.
const MAX_TITLE_CHARS: usize = 80;

/// The title prompt: a short label, nothing else.
const TITLE_SYSTEM: &str = "You name a chat thread. You are given the root message and its replies \
as 'Name: message' lines. Reply with a SHORT title of at most 6 words that captures what the thread \
is about. Output only the title - no quotation marks, trailing punctuation, label, or explanation. \
Use the thread's own language.";

/// Spawn thread-title generation when a thread may have just become eligible.
/// The cheap gate (a reply count + a title check) runs inside the task, off the
/// request path; the send never waits on it.
pub fn maybe_title_thread(state: &AppState, room_id: i64, parent_id: i64) {
    if !state.llm_available() {
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_thread_title(&st, room_id, parent_id).await {
            tracing::warn!(error = %e, parent_id, "thread title generation failed");
        }
    });
}

/// Generate + store a thread's title if eligible (at least [`MIN_REPLIES`] and no
/// title yet). Returns the stored title, or None when skipped. Idempotent: the
/// stored title short-circuits a re-run. A no-op without an LLM.
pub async fn run_thread_title(
    state: &AppState,
    room_id: i64,
    parent_id: i64,
) -> Result<Option<String>, AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Ok(None);
    };
    // Only ever title once.
    if db::chat::get_thread_title(&state.chat, parent_id)
        .await?
        .is_some()
    {
        return Ok(None);
    }
    if db::chat::count_replies(&state.chat, parent_id).await? < MIN_REPLIES {
        return Ok(None);
    }
    let Some(root) = db::chat::get_message(&state.chat, parent_id).await? else {
        return Ok(None);
    };
    // Guard against a bad caller: must be a real thread root in this room.
    if root.room_id != room_id || root.parent_id.is_some() {
        return Ok(None);
    }

    let mut msgs = vec![root];
    msgs.extend(db::chat::list_thread_replies(&state.chat, parent_id).await?);
    let labels = summary::author_labels(state, &msgs).await?;
    let text = summary::build_prompt_text(&msgs, &labels);
    if text.trim().is_empty() {
        return Ok(None);
    }

    let raw = match llm.complete_guarded(TITLE_SYSTEM, &text).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, parent_id, "thread title llm failed");
            return Ok(None);
        }
    };
    let title = clean_title(&raw);
    if title.is_empty() {
        return Ok(None);
    }
    db::chat::set_thread_title(&state.chat, parent_id, &title).await?;
    Ok(Some(title))
}

/// Normalize the model's answer into a tidy one-line title: the first non-empty
/// line, stripped of wrapping quotes and trailing punctuation, length-capped.
fn clean_title(raw: &str) -> String {
    let mut line = raw
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    // Strip wrapping quotes and trailing punctuation until stable, so an answer
    // like `"Title."` or `` `Title`! `` reduces cleanly regardless of order.
    loop {
        let before = line.clone();
        line = line
            .trim_matches(|c| c == '"' || c == '\'' || c == '`')
            .trim()
            .trim_end_matches(['.', '!', '?', ':'])
            .trim()
            .to_string();
        if line == before {
            break;
        }
    }
    line.chars().take(MAX_TITLE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::clean_title;

    #[test]
    fn clean_title_strips_quotes_punctuation_and_extra_lines() {
        assert_eq!(clean_title("\"Deploy plan\""), "Deploy plan");
        assert_eq!(clean_title("Release timing."), "Release timing");
        assert_eq!(
            clean_title("  Onboarding flow  \n\nmore"),
            "Onboarding flow"
        );
        assert_eq!(clean_title("`Budget review`!"), "Budget review");
        assert_eq!(clean_title(""), "");
        // Length cap.
        assert_eq!(clean_title(&"x".repeat(200)).chars().count(), 80);
    }
}
