//! LC-665: scheduled per-room AI activity digest.
//!
//! A slow background tick (`main::spawn_room_digest_dispatcher`) posts a
//! once-per-interval recap of a busy room's recent activity as the `assistant`
//! bot. Distinct from [`crate::digest`], which emails offline users their
//! unread activity; this posts an AI summary IN the channel.
//!
//! Opt-in per room (`rooms.digest_enabled`, a manager toggle) and deduped via
//! `rooms.digest_last_at` so a restart or a fast tick never double-posts. Async
//! by nature - nobody is waiting - which suits the multi-second local model. A
//! no-op when no LLM is configured.

use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// Post a digest at most once per this many hours per room.
const DIGEST_INTERVAL_HOURS: i64 = 24;
/// Don't post a digest for fewer than this many real messages since the last
/// one - a near-silent room does not need a recap.
const MIN_MESSAGES: usize = 5;
/// Cap on messages scanned per room (mirrors the catch-me-up bound).
const MAX_MESSAGES: i64 = 300;

/// The digest system prompt. Framed like the chat catch-up but for a channel's
/// day rather than one viewer's unread range.
const DIGEST_SYSTEM: &str = "You write a short daily digest of a team chat channel for members \
catching up. You are given recent messages as 'Name: message' lines. Reply in GitHub-flavored \
markdown: a handful of short bullets covering the key topics, decisions, and anything that needs \
follow-up. Be concise and faithful to the messages; do not invent details. If little of note \
happened, say so in one line.";

#[derive(Default, Debug, PartialEq, Eq)]
pub struct DigestStats {
    /// Rooms that were due and evaluated this tick.
    pub evaluated: usize,
    /// Rooms an actual digest was posted to.
    pub posted: usize,
}

/// One digest sweep: post due rooms' digests. Safe to call on a timer. A no-op
/// (returns zeroed stats) when no LLM is configured. Per-room failures are
/// logged and skipped so one bad room never stalls the sweep.
pub async fn run_digest_tick(state: &AppState) -> Result<DigestStats, AppError> {
    let mut stats = DigestStats::default();
    let Some(llm) = state.llm_client.clone() else {
        return Ok(stats);
    };
    // LC-679: the runtime kill switch also halts the room-digest tick.
    if !crate::routes::ai_gate::flag_on(state).await {
        return Ok(stats);
    }

    let due = db::chat::rooms_due_for_digest(&state.chat, DIGEST_INTERVAL_HOURS).await?;
    for room_id in due {
        stats.evaluated += 1;
        if let Err(e) = post_room_digest(state, &*llm, room_id, &mut stats).await {
            tracing::warn!(error = %e, room_id, "room digest failed; skipping");
        }
    }
    Ok(stats)
}

/// Evaluate and (if warranted) post one room's digest. Bumps `digest_last_at`
/// first - on every evaluation, posted or not - so a crash mid-post or a quiet
/// room never causes a re-run before the next interval.
async fn post_room_digest(
    state: &AppState,
    llm: &dyn crate::llm::LlmClient,
    room_id: i64,
    stats: &mut DigestStats,
) -> Result<(), AppError> {
    let since = db::chat::get_room_digest_last_at(&state.chat, room_id).await?;
    db::chat::set_room_digest_last_at(&state.chat, room_id).await?;

    let recent = db::chat::list_recent_messages(&state.chat, room_id, MAX_MESSAGES).await?;
    // Window to messages posted since the last digest (created_at is ISO-8601,
    // so lexical comparison is chronological). First run covers the recent tail.
    let mut msgs: Vec<_> = match since.as_deref() {
        Some(watermark) => recent
            .into_iter()
            .filter(|m| m.created_at.as_str() > watermark)
            .collect(),
        None => recent,
    };
    let real = msgs
        .iter()
        .filter(|m| !m.is_system && !m.body.trim().is_empty())
        .count();
    if real < MIN_MESSAGES {
        return Ok(());
    }
    msgs.reverse(); // list_recent_messages is newest-first; the prompt reads chronologically.

    let labels = crate::routes::summary::author_labels(state, &msgs).await?;
    let text = crate::routes::summary::build_prompt_text(&msgs, &labels);
    if text.trim().is_empty() {
        return Ok(());
    }
    let md = match llm.complete_guarded(DIGEST_SYSTEM, &text).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, room_id, "room digest summarization failed");
            return Ok(());
        }
    };

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let bot = crate::routes::assistant::assistant_bot(state).await?;
    let body = format!("**\u{1F5D3}\u{FE0F} Daily digest**\n\n{md}");
    let new_id = db::chat::insert_message(&state.chat, room_id, &bot.id, &body).await?;
    crate::routes::room::finalize_message_send(state, &room, &bot, new_id, &body, None).await?;
    stats.posted += 1;
    Ok(())
}
