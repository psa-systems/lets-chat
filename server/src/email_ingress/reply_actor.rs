//! LC-77-REPLY stage 2: actor for posting a reply-by-email message as
//! the real user identified by a resolved [`ReplyTokenRow`].
//!
//! Differs from [`super::actor`] (the email-inbox synthetic-actor path) in
//! three ways:
//!
//! 1. The post is authored by a real user (`user_id` from the reply
//!    token), NOT by the email-inbox synthetic actor.
//! 2. The room is derived from the original message that the user is
//!    replying to (`message_id` from the reply token), NOT from a
//!    per-room inbox config row.
//! 3. The body is the email reply with the quoted-original and
//!    signature stripped via [`strip_quoted_reply`]; the original is
//!    NOT auto-quoted into the new message (a chat reply is a sibling,
//!    not a thread reply; threading mid-chat would surprise other room
//!    participants who never saw the email round trip).
//!
//! The actor reuses the existing post-path machinery
//! ([`crate::routes::room::finalize_message_send`]) so a reply-by-email
//! post is indistinguishable from an HTTP-form post at the broadcast
//! layer: same `ChatEvent::NewMessage` shape, same mention-reconcile,
//! same outgoing-webhook dispatch, same per-user mention notification
//! email (recursion is broken by the recipient's
//! `Auto-Submitted: auto-generated` outbound on the next round; see
//! `crate::email::notification`).
//!
//! Posting gates mirror the HTTP `post_message` path: banned/muted
//! check, `is_room_accessible`, `can_post_with_policy`, DM-block check,
//! per-user `RateLimitKind::Message` cap. Any gate failure drops the
//! reply with a specific [`super::DropReason`] so an operator log can
//! distinguish "user replied from a quarantined account" from "user
//! replied to a room they were removed from."
//!
//! Token consumption: a reply token is one-shot. The actor deletes the
//! token row only after a successful post; gate failures leave the
//! token in place so a fixable error (rate limit, transient room state)
//! does not burn the user's reply window.

use crate::db::reply_tokens::ReplyTokenRow;
use crate::db::{self};
use crate::rate_limit::{Outcome as RlOutcome, RateLimitKind};
use crate::state::AppState;

use super::DropReason;

/// Outcome of one reply-by-email post attempt. The poll loop translates
/// this into the structured drop log + counter bump.
#[derive(Debug)]
pub enum ReplyOutcome {
    Posted { message_id: i64 },
    Dropped { reason: DropReason, detail: String },
}

/// Post a reply-by-email message as the real user identified by the
/// resolved token row. `email_body` is the already-extracted text/plain
/// body from [`super::parse::extract_body`] (the subject prefix and
/// MIME unwrap have already run). The actor strips the quoted-original
/// and signature, applies the HTTP post-path gates, inserts the row,
/// runs the normal finalize broadcast, and consumes the reply token.
pub async fn post_reply_message(
    state: &AppState,
    token_row: &ReplyTokenRow,
    raw_token: &str,
    email_body: &str,
) -> ReplyOutcome {
    // 1. Resolve the original message to its room.
    let original = match db::chat::get_message(&state.chat, token_row.message_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            // The original was deleted between mint and reply. Cascade
            // should have reaped the token, but a race is possible.
            return ReplyOutcome::Dropped {
                reason: DropReason::AddressNoMatch,
                detail: format!(
                    "reply-token references message {} which no longer exists",
                    token_row.message_id
                ),
            };
        }
        Err(e) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("get_message: {e}"),
            };
        }
    };

    let room = match db::chat::get_room(&state.chat, original.room_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::AddressNoMatch,
                detail: format!(
                    "reply-token's original message lives in room {} which no longer exists",
                    original.room_id
                ),
            };
        }
        Err(e) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("get_room: {e}"),
            };
        }
    };

    // 2. Resolve the replying user. is_banned / is_muted come from the
    // shared `User` shape; the find_user_by_id path returns the same
    // row the HTTP `AuthUser` extractor produces, so the gates below
    // are point-for-point what the HTTP post path would have enforced.
    let user: crate::models::user::User =
        match db::auth::find_user_by_id(&state.auth, &token_row.user_id).await {
            Ok(Some(u)) => u.into(),
            Ok(None) => {
                return ReplyOutcome::Dropped {
                    reason: DropReason::AddressNoMatch,
                    detail: format!(
                        "reply-token references user_id {} which no longer exists",
                        token_row.user_id
                    ),
                };
            }
            Err(e) => {
                return ReplyOutcome::Dropped {
                    reason: DropReason::InternalError,
                    detail: format!("find_user_by_id: {e}"),
                };
            }
        };

    if user.is_banned || user.is_muted {
        return ReplyOutcome::Dropped {
            reason: DropReason::InternalError,
            detail: format!(
                "user {} cannot post (banned={}, muted={})",
                user.id, user.is_banned, user.is_muted
            ),
        };
    }

    // 3. Per-user message rate limit (same cap the HTTP path reads).
    let msg_cap = crate::rate_limit::read_u32_setting(&state.settings, "rate_limit_messages").await;
    if let RlOutcome::Deny { retry_after } =
        state
            .rate_limits
            .check(RateLimitKind::Message, &user.id, msg_cap)
    {
        return ReplyOutcome::Dropped {
            reason: DropReason::RateLimited,
            detail: format!(
                "user {} exceeded message rate cap (retry_after {}s)",
                user.id, retry_after
            ),
        };
    }

    // 4. Room access + posting-policy gates. is_admin matches the HTTP
    // path: site admins can post in any non-DM room. DMs always require
    // explicit room membership.
    let is_admin = user.role == "admin";
    match db::chat::is_room_accessible(&state.chat, room.id, &user.id, is_admin).await {
        Ok(true) => {}
        Ok(false) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::AddressNoMatch,
                detail: format!("user {} no longer has access to room {}", user.id, room.id),
            };
        }
        Err(e) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("is_room_accessible: {e}"),
            };
        }
    }
    match crate::routes::room::can_post_with_policy(
        state,
        &user,
        room.id,
        &room.posting_allowed_for,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::AddressNoMatch,
                detail: format!(
                    "user {} cannot post in room {} per posting policy {}",
                    user.id, room.id, room.posting_allowed_for
                ),
            };
        }
        Err(e) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("can_post_with_policy: {e}"),
            };
        }
    }

    // 5. DM block check. Replies to a DM where either side has blocked
    // the other are silently dropped, same as the HTTP path.
    if room.room_type == "dm" {
        let members = match db::chat::list_room_member_ids(&state.chat, room.id).await {
            Ok(m) => m,
            Err(e) => {
                return ReplyOutcome::Dropped {
                    reason: DropReason::InternalError,
                    detail: format!("list_room_member_ids: {e}"),
                };
            }
        };
        if let Some(peer_id) = members.iter().find(|id| **id != user.id) {
            match db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await {
                Ok(true) => {
                    return ReplyOutcome::Dropped {
                        reason: DropReason::AddressNoMatch,
                        detail: format!("DM blocked between {} and {}", user.id, peer_id),
                    };
                }
                Ok(false) => {}
                Err(e) => {
                    return ReplyOutcome::Dropped {
                        reason: DropReason::InternalError,
                        detail: format!("is_blocked_either_way: {e}"),
                    };
                }
            }
        }
    }

    // 6. Strip quoted-original + signature, then validate length.
    let body = strip_quoted_reply(email_body);
    let body = body.trim();
    if body.is_empty() {
        return ReplyOutcome::Dropped {
            reason: DropReason::ParseFail,
            detail: "reply body empty after quote/signature strip".to_string(),
        };
    }
    if body.chars().count() > MAX_REPLY_CHARS {
        return ReplyOutcome::Dropped {
            reason: DropReason::ParseFail,
            detail: format!(
                "reply body {} chars exceeds {} cap after strip",
                body.chars().count(),
                MAX_REPLY_CHARS
            ),
        };
    }

    // 7. Insert the message row + run the standard finalize broadcast.
    // We pin the room from the original; no quote_id is constructed (the
    // email reply does not auto-thread on the original message).
    let new_id = match db::chat::insert_message(&state.chat, room.id, &user.id, body).await {
        Ok(id) => id,
        Err(e) => {
            return ReplyOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("insert_message: {e}"),
            };
        }
    };

    if let Err(e) =
        crate::routes::room::finalize_message_send(state, &room, &user, new_id, body).await
    {
        // Row is committed; broadcast/mention reconcile may be lossy.
        // Same shape as the email-inbox actor's tolerance for finalize
        // failures: the next page load shows the message.
        tracing::warn!(
            target: "email_ingress::reply",
            user_id = %user.id,
            room_id = room.id,
            message_id = new_id,
            error = ?e,
            "reply-by-email post-insert finalize failed; row committed",
        );
    }

    // 8. Consume the token (one-shot). Failure here is non-fatal: the
    // post succeeded, the next sweep will reap the row, and even if a
    // forwarded notification email retries, the original message_id +
    // user_id binding limits replay impact. Logged so an operator can
    // diagnose if it ever happens.
    match db::reply_tokens::consume(&state.chat, raw_token).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                target: "email_ingress::reply",
                token_user_id = %token_row.user_id,
                message_id = new_id,
                "reply-token consume returned false (already consumed?)",
            );
        }
        Err(e) => {
            tracing::warn!(
                target: "email_ingress::reply",
                token_user_id = %token_row.user_id,
                message_id = new_id,
                error = %e,
                "reply-token consume failed; token remains in DB until TTL sweep",
            );
        }
    }

    ReplyOutcome::Posted { message_id: new_id }
}

/// Hard cap on the reply body after strip. Matches the chat post path's
/// `MAX_MESSAGE_CHARS` cap so a reply-by-email can never produce a row
/// larger than the HTTP path would accept.
pub const MAX_REPLY_CHARS: usize = 16_000;

/// Strip the quoted original and the signature from an email reply
/// body. Returns the user's new text only.
///
/// Heuristics (in order):
///
/// 1. Cut at the first occurrence of an RFC 3676 signature delimiter
///    (`-- ` on a line by itself, with the trailing space required by
///    the RFC). Everything below is the user's `.signature`.
/// 2. Cut at the first line matching a common quote-intro pattern
///    (`On ... wrote:`). Everything below is the quoted original.
/// 3. After the above cuts, drop a trailing block of `>`-prefixed
///    quote lines (with their preceding blank line).
///
/// This is intentionally conservative: a strip we miss leaves more
/// content in the chat row (degraded UX, user can edit); an
/// over-aggressive strip would lose the user's real reply. We err on
/// the side of leaving extra text.
pub fn strip_quoted_reply(body: &str) -> String {
    let mut cut_at: Option<usize> = None;

    for (line_start, line) in line_offsets(body) {
        // RFC 3676 sigsep: literally `-- ` (dash-dash-space) on its own
        // line. We compare against the stripped-CR form because mail
        // bodies routinely carry CRLF.
        if line.trim_end_matches('\r') == "-- " {
            cut_at = Some(line_start);
            break;
        }
        // Common reply intro: `On {date}, {sender} wrote:` (Gmail,
        // Apple Mail) or `On {date} at {time}, {sender} ... wrote:`
        // (Gmail's longer form). The pattern is stable enough that a
        // simple prefix-and-suffix check catches the common case
        // without a regex dependency.
        let trimmed = line.trim_end_matches('\r').trim();
        if trimmed.starts_with("On ")
            && (trimmed.ends_with("wrote:") || trimmed.ends_with("wrote :"))
        {
            cut_at = Some(line_start);
            break;
        }
    }

    let head = match cut_at {
        Some(pos) => &body[..pos],
        None => body,
    };

    // Drop a trailing run of `>`-prefixed quote lines (the user's MUA
    // may have left an inline-quoted block at the end with no intro
    // line). The lines may also be `> >` (nested quote) so we accept
    // a leading `>` of any depth.
    let mut lines: Vec<&str> = head.lines().collect();
    while let Some(last) = lines.last() {
        let t = last.trim_end_matches('\r');
        if t.is_empty() || t.starts_with('>') {
            lines.pop();
        } else {
            break;
        }
    }
    lines.join("\n")
}

/// Iterate over (byte_offset_of_line_start, line_str) for each line of
/// `s`. Mirrors `str::lines` but exposes byte offsets so we can slice
/// `s[..pos]` at the start of a matching line.
fn line_offsets(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    s.split('\n').map(move |line| {
        let start = offset;
        offset += line.len() + 1; // +1 for the consumed '\n'
        (start, line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_keeps_body_when_no_sig_or_quote() {
        let body = "hi alice\n\nthanks for the ping";
        assert_eq!(strip_quoted_reply(body), body);
    }

    #[test]
    fn strip_cuts_at_rfc_3676_sigsep() {
        let body = "the actual reply\n\n-- \nbob | bob@example.com | (555) 0100";
        let out = strip_quoted_reply(body);
        assert_eq!(out, "the actual reply");
    }

    #[test]
    fn strip_cuts_at_gmail_reply_intro() {
        let body = "my response is brief\n\nOn Mon, May 25, 2026 at 10:00 AM, alice <alice@x.com> wrote:\n> original message here\n> more original";
        let out = strip_quoted_reply(body);
        assert_eq!(out, "my response is brief");
    }

    #[test]
    fn strip_cuts_short_on_wrote_form() {
        let body = "ok\n\nOn 5/25, alice wrote:\n> sup";
        let out = strip_quoted_reply(body);
        assert_eq!(out, "ok");
    }

    #[test]
    fn strip_drops_trailing_quote_block_without_intro() {
        // MUA stripped the intro line but left the inline quote.
        let body = "thanks\n\n> the original line\n> more original";
        let out = strip_quoted_reply(body);
        assert_eq!(out, "thanks");
    }

    #[test]
    fn strip_tolerates_crlf_line_endings() {
        let body = "ok\r\n\r\n-- \r\nbob";
        let out = strip_quoted_reply(body);
        assert_eq!(out, "ok");
    }

    #[test]
    fn strip_does_not_cut_within_a_line_starting_with_dash_dash() {
        // The sigsep is `-- ` (dash-dash-space) ON ITS OWN LINE. A
        // line like `-- and now this` is NOT a sigsep.
        let body = "before\n-- and now this\nafter";
        assert_eq!(strip_quoted_reply(body), body);
    }

    #[test]
    fn strip_preserves_inline_quote_followed_by_more_text() {
        // The user inline-quoted then wrote more text after. The
        // trailing-`>` drop must NOT eat the user's added content.
        let body = "> the original\n\ngreat point, i agree";
        let out = strip_quoted_reply(body);
        assert!(out.contains("great point, i agree"));
    }
}
