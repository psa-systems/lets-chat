//! LC-77 synthetic-actor post orchestration. Given a resolved
//! [`EmailInboxAuth`] and an extracted body, this module:
//!
//! 1. Inserts a row in `messages` via [`crate::db::chat::insert_email_inbox_message`].
//! 2. Calls [`crate::routes::room::finalize_email_inbox_message_send`] to
//!    broadcast `ChatEvent::NewMessage` to room subscribers and resolve
//!    @mentions into Mentioned events.
//! 3. Best-effort bumps `last_used_at` on the inbox.
//!
//! Failures are converted into the shared [`super::DropReason`] taxonomy so
//! the poll loop's always-Seen failure log emits a consistent shape.

use crate::db;
use crate::db::email_inbox::EmailInboxAuth;
use crate::error::AppError;
use crate::state::AppState;

use super::DropReason;

/// Outcome of attempting to post one polled message. The poll loop logs
/// `Dropped` cases with the shared [`super::DropReason`] taxonomy and
/// continues; `Posted` carries the new message id for the trace log.
#[derive(Debug)]
pub enum PostOutcome {
    Posted { message_id: i64 },
    Dropped { reason: DropReason, detail: String },
}

/// Post a resolved + parsed message as the email-ingress synthetic actor.
/// The `body` argument is the already-composed chat body (subject prefix
/// plus body, capped and truncated as needed by [`super::parse::extract_body`]).
///
/// The caller has already:
/// - resolved the inbox via [`super::resolve::resolve_inbox`]
/// - confirmed the inbox is not revoked
/// - checked the per-inbox rate limit
/// - run the loop-detection header heuristic
///
/// This function does NOT re-check those gates; it is intentionally narrow.
pub async fn post_email_message(
    state: &AppState,
    inbox: &EmailInboxAuth,
    body: &str,
) -> PostOutcome {
    if body.is_empty() {
        return PostOutcome::Dropped {
            reason: DropReason::ParseFail,
            detail: "empty body after parse".to_string(),
        };
    }

    let new_id = match db::chat::insert_email_inbox_message(
        &state.chat,
        inbox.room_id,
        inbox.id,
        body,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return PostOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("insert_email_inbox_message: {e}"),
            };
        }
    };

    let room = match db::chat::get_room(&state.chat, inbox.room_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return PostOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("room {} not found post-insert", inbox.room_id),
            };
        }
        Err(e) => {
            return PostOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("get_room: {e}"),
            };
        }
    };

    if let Err(e) = crate::routes::room::finalize_email_inbox_message_send(
        state,
        &room,
        inbox.id,
        &inbox.name,
        new_id,
        body,
    )
    .await
    {
        // The message row is committed; we still log the broadcast/mention
        // failure but consider the post successful from the caller's
        // perspective (next page load shows the message, mention table
        // may be missing but is best-effort already in the LC-74 path).
        match e {
            AppError::Internal(msg) => tracing::warn!(
                target: "email_ingress",
                inbox_id = inbox.id,
                message_id = new_id,
                error = %msg,
                "post-insert finalize failed; row committed, broadcast may be lossy",
            ),
            other => tracing::warn!(
                target: "email_ingress",
                inbox_id = inbox.id,
                message_id = new_id,
                error = ?other,
                "post-insert finalize failed; row committed, broadcast may be lossy",
            ),
        }
    }

    // Best-effort: bump last_used_at so the admin UI shows freshness without
    // a full SELECT-then-UPDATE round trip. Failure is non-fatal.
    if let Err(e) = db::email_inbox::touch_last_used(&state.chat, inbox.id).await {
        tracing::warn!(
            target: "email_ingress",
            inbox_id = inbox.id,
            error = %e,
            "touch_last_used failed (non-fatal)",
        );
    }

    PostOutcome::Posted { message_id: new_id }
}
