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

use super::attachments;
use super::parse::RawAttachment;
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
/// plus body, capped and truncated as needed by [`super::parse::extract_body`]);
/// `attachments` is the candidate set from
/// [`super::parse::extract_attachments`]. Each attachment goes through
/// the upload pipeline ([`super::attachments::process_attachment`])
/// before the message row is inserted; pipeline failures are non-fatal
/// to the parent message (the body still posts and the bad attachment
/// is logged INFO with its `AttachmentDrop` reason).
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
    attachments_in: &[RawAttachment],
) -> PostOutcome {
    if body.is_empty() && attachments_in.is_empty() {
        return PostOutcome::Dropped {
            reason: DropReason::ParseFail,
            detail: "empty body after parse and no attachments".to_string(),
        };
    }

    // Run the attachment pipeline FIRST so any pipeline failure is
    // logged and counted before we commit the message row. A pipeline
    // drop never aborts the post (the body is still useful chat content);
    // it just means the chat row has fewer attachments than the email did.
    let mut upload_ids: Vec<i64> = Vec::with_capacity(attachments_in.len());
    for raw in attachments_in {
        match attachments::process_attachment(state, raw).await {
            Ok(id) => upload_ids.push(id),
            Err(drop) => {
                tracing::info!(
                    target: "email_ingress::attachment_drop",
                    inbox_id = inbox.id,
                    filename = %raw.filename,
                    claimed_content_type = %raw.claimed_content_type,
                    reason = drop.as_str(),
                    detail = %drop.detail(),
                    "email ingress dropped attachment",
                );
            }
        }
    }

    // Decide the body now: an attachment-only email (subject empty, body
    // empty, but a successfully-uploaded attachment) renders as a body
    // of " " (a single space) so the markdown pipeline produces a valid
    // row; the attachment partial fills the visual space. If no
    // attachments succeeded AND body is empty after parse, we dropped
    // above.
    let body_for_insert = if body.is_empty() && !upload_ids.is_empty() {
        " "
    } else {
        body
    };

    let new_id = match db::chat::insert_email_inbox_message(
        &state.chat,
        inbox.room_id,
        inbox.id,
        body_for_insert,
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

    // Link every successfully-uploaded attachment to the new message
    // row. Failures here are best-effort (the upload row exists, just
    // the link is missing); the orphan sweeper would eventually reap a
    // long-orphan upload, but a fresh link failure leaves the file on
    // disk visible only via direct DB query.
    for upload_id in &upload_ids {
        if let Err(e) = db::uploads::link_upload_to_message(&state.chat, *upload_id, new_id).await {
            tracing::warn!(
                target: "email_ingress",
                upload_id = *upload_id,
                message_id = new_id,
                error = %e,
                "link_upload_to_message failed (attachment row committed without link)",
            );
        }
    }

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
