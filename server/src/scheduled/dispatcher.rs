//! LC-62: scheduled message dispatcher.
//!
//! Single-process invariant. The atomic claim (UPDATE ... WHERE delivered_at
//! IS NULL inside BEGIN IMMEDIATE, shared with the messages INSERT) prevents
//! double-delivery within one process. If this ever moves to a sidecar, the
//! per-row transaction and BEGIN IMMEDIATE are load-bearing under
//! concurrency, not decorative.
//!
//! Per-row flow:
//!
//! 1. Read-only re-validation (room still exists, author not banned / muted
//!    / kicked out, link filter still permits the body, DM peer hasn't
//!    blocked). On any deny, open a transaction, claim the row, set
//!    `failed_reason`, COMMIT, return `Dropped(reason)`. Rate-limit is NOT
//!    re-checked, by design: it was paid at schedule time.
//! 2. BEGIN IMMEDIATE + `try_claim` + INSERT INTO messages + COMMIT, all
//!    on one connection. On a lost claim, ROLLBACK and return `Retry`.
//!    Mention reconciliation and notification fan-out do NOT run inside
//!    this transaction.
//! 3. Best-effort post-commit: `link_upload_to_message` (if `file_id`),
//!    then `routes::room::finalize_message_send` (broadcast + mention
//!    reconcile + push). Errors logged, not rolled back; matches the
//!    live-send semantics where the messages row commits before WS / push
//!    fire.
//!
//! Mention resolution happens inside `finalize_message_send` (Option B per
//! the brainstorm): parser + resolver + reconcile + fanout all run at
//! delivery against the current users table. A `@bob` token whose target
//! user has renamed between schedule and delivery silently drops to
//! literal text, which the `/scheduled` UI (Task 4) surfaces by
//! re-rendering pending bodies through `views::markdown` against current
//! state.

use crate::db;
use crate::db::scheduled::ScheduledRow;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::state::AppState;

/// Cap on rows the tick will attempt per fire. A long server outage that
/// leaves a backlog of hundreds of due rows would otherwise stampede
/// push fan-out at restart; later ticks drain what is left.
const BATCH_SIZE: i64 = 100;

#[derive(Debug, Default, Clone, Copy)]
pub struct DispatchStats {
    pub delivered: u64,
    pub dropped: u64,
    pub retries: u64,
}

#[derive(Debug)]
pub enum DispatchOutcome {
    Delivered,
    Dropped(&'static str),
    /// Either someone else (another tick, another process) claimed the
    /// row first, or the row no longer matches the due predicate. Either
    /// way, this caller has nothing more to do.
    Retry,
}

/// Drain the due queue once. A per-row internal error bumps that row's
/// `attempt_count` and pushes `next_attempt_at` forward by exponential
/// backoff, then iteration continues with the next row; one bad row does
/// not stall the rest of the tick.
pub async fn run_dispatch_tick(state: &AppState) -> Result<DispatchStats, AppError> {
    let rows = db::scheduled::select_due(&state.chat, BATCH_SIZE).await?;
    let mut stats = DispatchStats::default();
    for row in rows {
        let id = row.id;
        let attempt_count = row.attempt_count;
        match dispatch_one(state, row).await {
            Ok(DispatchOutcome::Delivered) => stats.delivered += 1,
            Ok(DispatchOutcome::Dropped(reason)) => {
                stats.dropped += 1;
                tracing::info!(
                    scheduled_id = id,
                    reason,
                    "scheduled message dropped at delivery"
                );
            }
            Ok(DispatchOutcome::Retry) => stats.retries += 1,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    scheduled_id = id,
                    "scheduled dispatch failed; backing off"
                );
                let secs = backoff_secs(attempt_count);
                if let Err(e2) = db::scheduled::bump_attempt(&state.chat, id, secs).await {
                    tracing::warn!(
                        error = %e2,
                        scheduled_id = id,
                        "bump_attempt failed; row may retry on next tick"
                    );
                }
                stats.retries += 1;
            }
        }
    }
    Ok(stats)
}

/// Exponential backoff: 60s, 120s, 240s, ... 256 minutes (cap at
/// `attempt_count == 8`).
fn backoff_secs(attempt_count: i64) -> i64 {
    let n = attempt_count.clamp(0, 8) as u32;
    60 * (1i64 << n)
}

// Internal control-flow enum for `validate_delivery`. Pass is the common
// case; boxing the Room+User fields to satisfy `clippy::large_enum_variant`
// would trade a heap allocation per dispatch for a 568-byte stack saving
// on the rare Deny branch, which is the wrong direction.
#[allow(clippy::large_enum_variant)]
enum Validation {
    Pass { room: Room, user: User },
    Deny(&'static str),
}

async fn validate_delivery(state: &AppState, row: &ScheduledRow) -> Result<Validation, AppError> {
    let Some(room) = db::chat::get_room(&state.chat, row.room_id).await? else {
        return Ok(Validation::Deny("room no longer exists"));
    };
    let Some(user_record) = db::auth::find_user_by_id(&state.auth, &row.user_id).await? else {
        // purge_user_chat (Task 6) deletes scheduled rows in the same
        // transaction as the user delete, so this branch should be
        // unreachable in practice. Treat it as a drop rather than an Err
        // so the row does not loop forever on backoff.
        return Ok(Validation::Deny("author no longer exists"));
    };
    let user: User = user_record.into();
    if user.is_banned {
        return Ok(Validation::Deny("author is banned"));
    }
    if user.is_muted {
        return Ok(Validation::Deny("author is muted"));
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, row.room_id, &user.id, is_admin).await? {
        return Ok(Validation::Deny("author lost room access"));
    }
    if !crate::routes::room::can_post_with_policy(
        state,
        &user,
        row.room_id,
        &room.posting_allowed_for,
    )
    .await?
    {
        return Ok(Validation::Deny("author cannot post in this room"));
    }
    if room.room_type == "dm" {
        let members = db::chat::list_room_member_ids(&state.chat, row.room_id).await?;
        if let Some(peer_id) = members.iter().find(|id| **id != user.id) {
            if db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await? {
                return Ok(Validation::Deny("DM peer blocked author"));
            }
        }
    }
    let link_filter_on = db::settings::get_setting(&state.settings, "link_filter_enabled")
        .await?
        .as_deref()
        == Some("true");
    if link_filter_on {
        let hosts = crate::links::extract_hosts(&row.body);
        if let Some((rule, _host)) = db::anti_spam::find_match(&state.chat, &hosts).await? {
            if rule.action == db::anti_spam::FilterAction::Block {
                return Ok(Validation::Deny("body contains a blocked link"));
            }
        }
    }
    Ok(Validation::Pass { room, user })
}

async fn dispatch_one(state: &AppState, row: ScheduledRow) -> Result<DispatchOutcome, AppError> {
    let (room, user) = match validate_delivery(state, &row).await? {
        Validation::Pass { room, user } => (room, user),
        Validation::Deny(reason) => {
            // Claim + mark_dropped inside one transaction so a crash
            // between the two re-runs cleanly on the next tick.
            let mut conn = state.chat.acquire().await?;
            sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
            let claimed = match db::scheduled::try_claim(&mut conn, row.id).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    return Err(e.into());
                }
            };
            if !claimed {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Ok(DispatchOutcome::Retry);
            }
            if let Err(e) = db::scheduled::mark_dropped(&mut conn, row.id, reason).await {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(e.into());
            }
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            return Ok(DispatchOutcome::Dropped(reason));
        }
    };

    let mut conn = state.chat.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    let claimed = match db::scheduled::try_claim(&mut conn, row.id).await {
        Ok(c) => c,
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(e.into());
        }
    };
    if !claimed {
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        return Ok(DispatchOutcome::Retry);
    }

    let insert_result = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id, quote_id) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(row.room_id)
    .bind(&row.user_id)
    .bind(&row.body)
    .bind(row.parent_id)
    .bind(row.quote_id)
    .execute(&mut *conn)
    .await;

    let new_id = match insert_result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(e.into());
        }
    };

    sqlx::query("COMMIT").execute(&mut *conn).await?;
    drop(conn);

    // Best-effort post-commit. The orphan-sweep guard protects the upload
    // only while `scheduled_messages.delivered_at IS NULL`; once the
    // COMMIT above flips it, the sweep is free to delete the upload, so
    // the link UPDATE here may race with a sweep running in another
    // process. link_upload_to_message's WHERE clause accepts NULL or
    // already-equal message_id, so a stale file_id silently no-ops; a
    // genuinely missing upload is logged but not fatal (the message
    // ships text-only).
    if let Some(file_id) = row.file_id {
        if let Err(e) = db::uploads::link_upload_to_message(&state.chat, file_id, new_id).await {
            tracing::warn!(
                error = %e,
                file_id,
                message_id = new_id,
                scheduled_id = row.id,
                "failed to link upload to delivered scheduled message"
            );
        }
    }

    if let Err(e) =
        crate::routes::room::finalize_message_send(state, &room, &user, new_id, &row.body).await
    {
        tracing::warn!(
            error = %e,
            message_id = new_id,
            scheduled_id = row.id,
            "finalize_message_send failed after delivery; message exists, side effects partial"
        );
    }

    Ok(DispatchOutcome::Delivered)
}
