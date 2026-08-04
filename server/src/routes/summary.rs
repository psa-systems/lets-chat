//! LC-484: AI "catch me up" summaries for threads and channels.
//!
//! Reuses the operator LLM client (`crate::llm`, LC-396) with a chat-specific
//! system prompt. The generated markdown is rendered through the same
//! `views::markdown` pipeline as messages. No persistent cache: unlike an
//! immutable call transcript, a thread keeps growing and an unread range is
//! per-viewer and ephemeral, so a cached summary would go stale immediately.
//! Each click generates fresh (the regenerate button makes that explicit), the
//! same posture as the transcript "regenerate" action.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::db::chat::RawMessage;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::summary::{CatchMeUpPanel, SummaryFragment};
use crate::views::{html, Html};

/// Chat catch-up system prompt (distinct from the transcript prompt in
/// `crate::llm`). Same markdown shape so it renders through the message
/// pipeline, but framed for a missed conversation rather than a meeting.
const CHAT_SYSTEM_PROMPT: &str = "You help a chat user catch up on a conversation they missed. \
Reply in GitHub-flavored markdown with a short '## Summary' section (2-5 bullet points of what was \
discussed and decided) followed by an '## Action items' section as a bullet list of any tasks, \
questions, or follow-ups directed at people (write 'None.' if there are none). Be concise and \
faithful to the messages; do not invent details. Refer to people by the names shown.";

/// Cap on the prompt text fed to the model (mirrors the transcript handler).
const MAX_PROMPT_CHARS: usize = 48_000;
/// Cap on how many recent messages we scan for a channel summary.
const MAX_SUMMARY_MESSAGES: i64 = 300;
/// Fallback count when there is nothing unread (summarize the recent tail).
const RECENT_FALLBACK: i64 = 50;

/// `display_name` if non-empty, else `username` (the codebase-wide label rule).
fn label_for(username: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => username.to_string(),
    }
}

/// Resolve display labels for every distinct author across `msgs` in one bulk
/// auth query. Synthetic actors (webhook/bridge/email) fall back to their raw
/// user id, which is acceptable for prompt context.
pub(crate) async fn author_labels(
    state: &AppState,
    msgs: &[RawMessage],
) -> Result<HashMap<String, String>, AppError> {
    let mut unique: HashSet<&str> = HashSet::new();
    for m in msgs {
        unique.insert(m.user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let resolved = db::auth::display_names_for_ids(&state.auth, &ids).await?;
    Ok(resolved
        .into_iter()
        .map(|(id, (u, d))| (id, label_for(&u, d.as_deref())))
        .collect())
}

/// Build the "Label: body" prompt block (chronological), skipping system and
/// empty messages, bounded to `MAX_PROMPT_CHARS`.
pub(crate) fn build_prompt_text(msgs: &[RawMessage], labels: &HashMap<String, String>) -> String {
    let mut text = String::new();
    for m in msgs {
        if m.is_system {
            continue;
        }
        let body = m.body.trim();
        if body.is_empty() {
            continue;
        }
        let label = labels
            .get(&m.user_id)
            .cloned()
            .unwrap_or_else(|| m.user_id.clone());
        let line = format!("{label}: {body}\n");
        if text.len() + line.len() > MAX_PROMPT_CHARS {
            break;
        }
        text.push_str(&line);
    }
    text
}

async fn require_access(state: &AppState, user: &User, room_id: i64) -> Result<(), AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Shared: run the LLM over `msgs` and render the result fragment. `post_url` +
/// `target_id` wire the regenerate button back to the same container.
async fn run_summary(
    state: &AppState,
    msgs: &[RawMessage],
    post_url: String,
    target_id: String,
) -> Result<Html, AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "summarization is not configured".into(),
        ));
    };
    let labels = author_labels(state, msgs).await?;
    let text = build_prompt_text(msgs, &labels);
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("nothing to summarize".into()));
    }
    let md = match llm.complete_guarded(CHAT_SYSTEM_PROMPT, &text).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "chat summary failed");
            return Err(AppError::BadRequest("summarization failed".into()));
        }
    };
    let summary_html = crate::views::markdown::render(&md, &[], &[]);
    html(&SummaryFragment {
        summary_html,
        post_url,
        target_id,
    })
}

/// GET /room/{room_id}/summary - open the catch-me-up drawer (shared
/// `#thread-panel` slot). Shows a Generate button + the unread count.
pub async fn open_catch_me_up(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_access(&state, &user, room_id).await?;
    let unread_count = db::chat::get_unread_count(&state.chat, &user.id, room_id).await?;
    html(&CatchMeUpPanel {
        room_id,
        unread_count,
        has_unread: unread_count > 0,
    })
}

/// POST /room/{room_id}/summary - summarize the viewer's unread range (or the
/// recent tail when nothing is unread).
pub async fn summarize_channel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_access(&state, &user, room_id).await?;

    let watermark = db::chat::get_dm_read_state(&state.chat, &user.id, room_id)
        .await?
        .map(|s| s.last_read_message_id)
        .unwrap_or(0);
    let recent = db::chat::list_recent_messages(&state.chat, room_id, MAX_SUMMARY_MESSAGES).await?;
    let mut msgs: Vec<RawMessage> = recent.into_iter().filter(|m| m.id > watermark).collect();
    if msgs.is_empty() {
        // Nothing unread: summarize the recent tail so the action is never a
        // dead end.
        msgs = db::chat::list_recent_messages(&state.chat, room_id, RECENT_FALLBACK).await?;
    }
    msgs.reverse(); // list_recent_messages is newest-first; the prompt reads chronologically.

    run_summary(
        &state,
        &msgs,
        format!("/room/{room_id}/summary"),
        "room-summary-body".to_string(),
    )
    .await
}

/// POST /room/{room_id}/thread/{parent_id}/summary - summarize one thread
/// (root message + its replies).
pub async fn summarize_thread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, parent_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    require_access(&state, &user, room_id).await?;
    let parent = db::chat::get_message(&state.chat, parent_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if parent.room_id != room_id {
        return Err(AppError::NotFound);
    }
    let mut msgs = vec![parent];
    msgs.extend(db::chat::list_thread_replies(&state.chat, parent_id).await?);

    run_summary(
        &state,
        &msgs,
        format!("/room/{room_id}/thread/{parent_id}/summary"),
        format!("thread-summary-{parent_id}"),
    )
    .await
}
