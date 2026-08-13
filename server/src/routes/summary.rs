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
use crate::db::inbox::InboxRow;
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

/// LC-705: workspace catch-up system prompt. The per-channel grouping means the
/// summary spans many conversations, so it names channels where that helps,
/// unlike the single-conversation `CHAT_SYSTEM_PROMPT`.
const WORKSPACE_SYSTEM_PROMPT: &str = "You help a chat user catch up on everything they missed across \
multiple channels and direct messages while they were away. The conversations are grouped under \
'## <name>' headers. Reply in GitHub-flavored markdown: lead with a short '## Summary' section (2-6 \
bullet points of the most important things discussed or decided across all channels, naming the channel \
where it helps) followed by an '## Action items' section as a bullet list of any tasks, questions, or \
follow-ups directed at people (write 'None.' if there are none). Be concise and faithful to the messages; \
do not invent details. Refer to people by the names shown.";

/// Cap on the prompt text fed to the model (mirrors the transcript handler).
const MAX_PROMPT_CHARS: usize = 48_000;
/// Cap on how many recent messages we scan for a channel summary.
const MAX_SUMMARY_MESSAGES: i64 = 300;
/// Fallback count when there is nothing unread (summarize the recent tail).
const RECENT_FALLBACK: i64 = 50;
/// LC-705: how many recent unread messages (workspace-wide, newest-first) to
/// pull into the Home catch-up prompt before the `MAX_PROMPT_CHARS` cap trims.
const WORKSPACE_SCAN: i64 = 200;

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
    // LC-679: every caller of this helper is an LLM catch-up route, so fold in
    // the runtime flag + role gate here - 403 for an unprivileged/flag-off user.
    super::ai_gate::require_llm_in_room(state, room_id, user).await?;
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

/// LC-705: resolve display labels for every distinct author across unread inbox
/// rows in one bulk auth query (the `InboxRow` counterpart of `author_labels`).
async fn workspace_labels(
    state: &AppState,
    rows: &[InboxRow],
) -> Result<HashMap<String, String>, AppError> {
    let mut unique: HashSet<&str> = HashSet::new();
    for r in rows {
        unique.insert(r.author_user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let resolved = db::auth::display_names_for_ids(&state.auth, &ids).await?;
    Ok(resolved
        .into_iter()
        .map(|(id, (u, d))| (id, label_for(&u, d.as_deref())))
        .collect())
}

/// LC-705: build the cross-room catch-up prompt from unread inbox rows. Groups
/// by room in first-seen order (which is newest-activity first, since
/// `list_unread` is newest-first), each room's messages in chronological order
/// under a "## {room}" header as "Label: body" lines. Skips empty bodies and is
/// bounded to `MAX_PROMPT_CHARS`.
fn build_workspace_prompt(rows: &[InboxRow], labels: &HashMap<String, String>) -> String {
    use std::collections::hash_map::Entry;
    let mut order: Vec<i64> = Vec::new();
    let mut by_room: HashMap<i64, Vec<&InboxRow>> = HashMap::new();
    for r in rows {
        match by_room.entry(r.room_id) {
            Entry::Occupied(mut e) => e.get_mut().push(r),
            Entry::Vacant(e) => {
                order.push(r.room_id);
                e.insert(vec![r]);
            }
        }
    }
    let mut text = String::new();
    'rooms: for room_id in &order {
        let bucket = &by_room[room_id];
        let first = bucket[0];
        let name = first.room_name.trim();
        let header_name = if !name.is_empty() {
            name.to_string()
        } else if first.room_type == "dm" {
            "Direct message".to_string()
        } else {
            format!("Room {room_id}")
        };
        let header = format!("## {header_name}\n");
        if text.len() + header.len() > MAX_PROMPT_CHARS {
            break;
        }
        text.push_str(&header);
        // `list_unread` is newest-first; read the bucket chronologically.
        for m in bucket.iter().rev() {
            let body = m.body.trim();
            if body.is_empty() {
                continue;
            }
            let label = labels
                .get(&m.author_user_id)
                .cloned()
                .unwrap_or_else(|| m.author_user_id.clone());
            let line = format!("{label}: {body}\n");
            if text.len() + line.len() > MAX_PROMPT_CHARS {
                break 'rooms;
            }
            text.push_str(&line);
        }
        text.push('\n');
    }
    text
}

/// POST /home/summary - LC-705: one-click "Catch me up" across the whole
/// workspace. Gathers the viewer's recent unread from every accessible room and
/// DM (access enforced by `list_unread`), groups it per channel, and summarizes
/// in a single pass. Gated on the workspace AI flag + audience (LC-702/LC-705).
/// No cache, like the per-room summary: each click regenerates.
pub async fn summarize_home(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    super::ai_gate::require_llm_workspace(&state, &user).await?;

    let post_url = "/home/summary".to_string();
    let target_id = "home-summary-body".to_string();

    let is_admin = user.role == "admin";
    let rows =
        db::inbox::list_unread(&state.chat, &user.id, is_admin, WORKSPACE_SCAN, None).await?;
    if rows.is_empty() {
        // The entry point is hidden when nothing is unread; this covers the race
        // where the last unread was read elsewhere between render and click. A
        // caught-up note reads better than an error.
        let md = crate::i18n::translate_current("home-catchup-ai-caught-up");
        let summary_html = crate::views::markdown::render(&md, &[], &[]);
        return html(&SummaryFragment {
            summary_html,
            post_url,
            target_id,
        });
    }

    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "summarization is not configured".into(),
        ));
    };
    let labels = workspace_labels(&state, &rows).await?;
    let text = build_workspace_prompt(&rows, &labels);
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("nothing to summarize".into()));
    }
    let md = match llm.complete_guarded(WORKSPACE_SYSTEM_PROMPT, &text).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "workspace summary failed");
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

#[cfg(test)]
mod tests {
    use super::{build_workspace_prompt, InboxRow};
    use std::collections::HashMap;

    fn row(room_id: i64, room_name: &str, room_type: &str, author: &str, body: &str) -> InboxRow {
        InboxRow {
            message_id: 0,
            room_id,
            room_name: room_name.into(),
            room_type: room_type.into(),
            author_user_id: author.into(),
            body: body.into(),
            created_at: "2026-01-01 00:00:00".into(),
        }
    }

    #[test]
    fn workspace_prompt_groups_by_room_and_reads_chronologically() {
        let mut labels = HashMap::new();
        labels.insert("u1".to_string(), "Alice".to_string());
        // list_unread is newest-first: within a room the later message comes
        // first in the slice, so the prompt must reverse it back to chronological.
        let rows = vec![
            row(7, "general", "public", "u1", "second"),
            row(7, "general", "public", "u1", "first"),
            row(9, "random", "public", "u2", "hello"),
        ];
        let out = build_workspace_prompt(&rows, &labels);
        assert_eq!(
            out, "## general\nAlice: first\nAlice: second\n\n## random\nu2: hello\n\n",
            "rooms grouped in first-seen order, messages chronological, unknown \
             author falls back to its id"
        );
    }

    #[test]
    fn workspace_prompt_labels_dm_and_skips_empty_bodies() {
        let labels = HashMap::new();
        let rows = vec![
            row(3, "", "dm", "peer", "hi there"),
            row(3, "", "dm", "peer", "   "),
        ];
        let out = build_workspace_prompt(&rows, &labels);
        assert_eq!(out, "## Direct message\npeer: hi there\n\n");
    }
}
