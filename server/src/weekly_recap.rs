//! LC-671: personal weekly recap DM.
//!
//! An operator-opt-in weekly tick DMs each active user a short, AI-written recap
//! of their week (messages sent, kudos received) as the `assistant` bot.
//! Personal (only the recipient sees it), low-stakes, and async - a good
//! local-model fit. Off unless the operator sets `LETS_CHAT_WEEKLY_RECAP` (the
//! gate lives in the main.rs dispatcher), and a no-op without an LLM.
//!
//! Deduped via `users.last_weekly_recap_at`, bumped on every evaluation so a
//! restart or a fast tick never double-DMs within the 7-day window. A user whose
//! week was empty (no messages, no kudos) is skipped rather than nagged.

use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

/// The recap prompt: a warm, plain-text note built from the week's numbers.
const RECAP_SYSTEM: &str = "You write a short, warm weekly recap message for one chat user, sent as \
a friendly direct message. You are given their activity numbers for the past week. Write 2-3 \
encouraging sentences that celebrate their week - mention the kudos warmly if there are any - and \
keep it genuine, not corny. Plain text only: no markdown headings, no bullet lists, no preamble, no \
sign-off. Do not invent numbers or facts beyond what you are given.";

#[derive(Default, Debug, PartialEq, Eq)]
pub struct RecapStats {
    /// Users that were due and evaluated this tick.
    pub evaluated: usize,
    /// Users an actual recap DM was sent to.
    pub sent: usize,
}

/// One weekly-recap sweep. Safe to call on a timer. A no-op (zeroed stats) when
/// no LLM is configured. Per-user failures are logged and skipped so one bad
/// user never stalls the sweep.
pub async fn run_weekly_recap_tick(state: &AppState) -> Result<RecapStats, AppError> {
    let mut stats = RecapStats::default();
    let Some(llm) = state.llm_client.clone() else {
        return Ok(stats);
    };
    let candidates = db::auth::weekly_recap_candidates(&state.auth).await?;
    if candidates.is_empty() {
        return Ok(stats);
    }
    let bot = crate::routes::assistant::assistant_bot(state).await?;
    for uid in candidates {
        stats.evaluated += 1;
        // Bump first so a crash mid-send or a fast re-tick never double-DMs.
        db::auth::set_last_weekly_recap_at(&state.auth, &uid).await?;
        match send_recap(state, &*llm, &bot, &uid).await {
            Ok(true) => stats.sent += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, user_id = %uid, "weekly recap failed; skipping"),
        }
    }
    Ok(stats)
}

/// Compose + DM one user's recap. Returns whether a DM was sent (false = the
/// week was empty, the recap could not be composed, or the recipient is gone).
async fn send_recap(
    state: &AppState,
    llm: &dyn crate::llm::LlmClient,
    bot: &User,
    uid: &str,
) -> Result<bool, AppError> {
    let messages = db::stats::weekly_message_count(&state.chat, uid).await?;
    let kudos = db::stats::weekly_kudos_received(&state.chat, uid).await?;
    if messages == 0 && kudos == 0 {
        return Ok(false); // nothing to celebrate - do not nag
    }
    let Some(recipient) = db::auth::find_user_by_id(&state.auth, uid).await? else {
        return Ok(false);
    };

    let prompt = format!("Messages sent this week: {messages}\nKudos received this week: {kudos}");
    let body = match llm.complete_guarded(RECAP_SYSTEM, &prompt).await {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            tracing::warn!(error = %e, user_id = %uid, "weekly recap compose failed");
            return Ok(false);
        }
    };
    if body.is_empty() {
        return Ok(false);
    }

    // Deliver as a DM from the assistant bot (reuse the existing 1:1 DM if any).
    let room = match db::chat::find_dm_room(&state.chat, &bot.id, uid).await? {
        Some(r) => r,
        None => {
            let name = format!("@{}", recipient.username);
            db::chat::create_dm_room(&state.chat, &name, &bot.id, uid).await?
        }
    };
    let new_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &body).await?;
    crate::routes::room::finalize_message_send(state, &room, bot, new_id, &body, None).await?;
    Ok(true)
}
