//! LC-77-REPLY stage 1: dispatch a per-message mention or DM notification
//! email. Fired from the mention-reconcile hook in `routes::room` for each
//! newly-added mention (or DM recipient).
//!
//! The dispatcher gates on (in order, short-circuit on first miss):
//!
//! 1. Recipient has a verified email address.
//! 2. Recipient has `notify_email_activity_enabled = 1`.
//! 3. SMTP mailer is configured (`state.mailer.is_some()`).
//! 4. Per-recipient rate limit (`RateLimitKind::EmailMentionNotification`,
//!    20/minute).
//! 5. Original message still exists (race: the mention path can interleave
//!    with a delete).
//!
//! On all-gates-pass it mints a reply token via [`db::reply_tokens`],
//! constructs `Reply-To: reply-<token>@<ingress-domain>` when the ingress
//! domain is configured, renders both template parts, and submits via
//! [`crate::mail::Mailer::send_multipart_with_reply_to`].
//!
//! Returns [`DispatchOutcome`] so callers (and tests) can distinguish
//! gating decisions from send-side errors without parsing log lines.

use askama::Template;

use crate::db;
use crate::rate_limit::{Outcome as RlOutcome, RateLimitKind};
use crate::state::AppState;
use crate::views::email_notification::{NotificationHtml, NotificationText};

/// Kind of activity that triggered the notification email. Drives the
/// subject line + body's intro line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    /// `@username` mention in a room (room_type = "public" or "private").
    Mention,
    /// Implicit DM ping (room_type = "dm").
    Dm,
}

/// Outcome of one dispatch attempt. Variants are mutually exclusive and
/// short-circuit at the first failed gate; the dispatcher never falls
/// through after returning. Tests assert on the variant directly so the
/// gate ordering can be inspected without parsing logs.
#[derive(Debug)]
pub enum DispatchOutcome {
    SkippedNoMailer,
    SkippedNoEmail,
    SkippedUnverified,
    SkippedOptOut,
    SkippedRateLimit,
    /// Recipient lookup failed (no user row). Race with user delete.
    SkippedNoRecipient,
    /// Original message lookup failed (no row). Race with message delete.
    SkippedNoMessage,
    /// Sender lookup failed (no user row OR webhook author with no usable
    /// display). The notification still wouldn't carry useful context;
    /// skip rather than send "unknown mentioned you".
    SkippedNoSender,
    /// Template render returned an error. Distinct from a send error so
    /// the operator log distinguishes "Askama failed" from "SMTP failed."
    RenderFailed(String),
    /// Mailer.send_multipart_with_reply_to returned Err.
    SendFailed(String),
    /// All gates passed and SMTP submit returned Ok.
    Sent,
}

/// Per-user cap on outbound mention/DM notification emails per minute.
/// See `RateLimitKind::EmailMentionNotification`.
pub const RATE_PER_MIN: u32 = 20;

/// Reply-token TTL. Per the LC-77-REPLY brainstorm: 7 days. Lengthen on
/// operator feedback; shortening invalidates outstanding notifications.
pub const REPLY_TOKEN_TTL_HOURS: i64 = 7 * 24;

/// 140-char snippet length, matching the digest's [`crate::digest::build_snippet`].
const SNIPPET_MAX: usize = 140;

pub async fn dispatch_mention_notification(
    state: &AppState,
    recipient_user_id: &str,
    message_id: i64,
    kind: NotificationKind,
    room_id: i64,
    room_name: &str,
) -> DispatchOutcome {
    // 1. Recipient lookup + email/verified/opt-in gates.
    let recipient = match db::auth::find_user_by_id(&state.auth, recipient_user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return DispatchOutcome::SkippedNoRecipient,
        Err(e) => {
            tracing::warn!(
                target: "email::notification",
                user_id = recipient_user_id,
                error = %e,
                "find_user_by_id failed; skipping notification",
            );
            return DispatchOutcome::SkippedNoRecipient;
        }
    };
    let Some(email) = recipient.email.as_deref().filter(|s| !s.is_empty()) else {
        return DispatchOutcome::SkippedNoEmail;
    };
    let verified = match db::auth::get_user_email_verified_at(&state.auth, recipient_user_id).await
    {
        Ok(opt) => opt.is_some(),
        Err(_) => false,
    };
    if !verified {
        return DispatchOutcome::SkippedUnverified;
    }
    if !recipient.notify_email_activity_enabled {
        return DispatchOutcome::SkippedOptOut;
    }

    // 2. Mailer gate.
    let Some(mailer) = state.mailer.as_ref() else {
        return DispatchOutcome::SkippedNoMailer;
    };

    // 3. Per-recipient rate limit.
    if let RlOutcome::Deny { .. } = state.rate_limits.check(
        RateLimitKind::EmailMentionNotification,
        recipient_user_id,
        RATE_PER_MIN,
    ) {
        return DispatchOutcome::SkippedRateLimit;
    }

    // 4. Read the original message. Race: an earlier dispatch may have
    // landed between the mention being broadcast and this lookup; if the
    // message has been deleted, drop silently.
    let raw = match db::chat::get_message(&state.chat, message_id).await {
        Ok(Some(m)) => m,
        Ok(None) => return DispatchOutcome::SkippedNoMessage,
        Err(_) => return DispatchOutcome::SkippedNoMessage,
    };

    // 5. Sender display label. For real-user authored messages, look up the
    // sender. For webhook / email-inbox synthetic-actor messages, the
    // models::Message has author_name embedded in higher-level views; here
    // we have RawMessage which carries user_id + webhook_id + email_inbox_id.
    // Use the existing identity helpers; if everything is None and user_id
    // is empty, skip (we can't compose "unknown mentioned you").
    let sender_label = match resolve_sender_label(state, &raw).await {
        Some(s) => s,
        None => return DispatchOutcome::SkippedNoSender,
    };

    // 6. Reply-token mint + ingress-domain lookup. `expires_at` is a
    // literal UTC timestamp string in the same shape SQLite's
    // `datetime('now')` produces; the sweep query compares against
    // `datetime('now')` lexically so the format must match exactly.
    let token = db::reply_tokens::mint_token();
    let expires_at = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(REPLY_TOKEN_TTL_HOURS))
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    if let Err(e) = db::reply_tokens::insert(
        &state.chat,
        &token,
        recipient_user_id,
        message_id,
        &expires_at,
    )
    .await
    {
        return DispatchOutcome::SendFailed(format!("reply_tokens::insert: {e}"));
    }

    let ingress_domain = ingress_domain(state).await;
    let reply_to: Option<String> = ingress_domain
        .as_deref()
        .map(|d| format!("{}{token}@{d}", crate::email_ingress::resolve::REPLY_PREFIX));
    let reply_supported = reply_to.is_some();

    // 7. Compose subject + URLs + snippet (parallel text + html).
    let subject = match kind {
        NotificationKind::Mention => {
            format!("[lets-chat] {sender_label} mentioned you in #{room_name}")
        }
        NotificationKind::Dm => format!("[lets-chat] {sender_label} sent you a direct message"),
    };
    let message_url = format!("{}/room/{room_id}#msg-{message_id}", state.base_url);
    let settings_url = format!("{}/settings", state.base_url);
    let (snippet_plain, snippet_html) = crate::digest::build_snippet(&raw.body);
    let snippet_plain = truncate_snippet(&snippet_plain);
    let snippet_html = truncate_snippet(&snippet_html);

    let html_view = NotificationHtml {
        sender_label: &sender_label,
        room_label: room_name,
        message_url: &message_url,
        snippet_html: &snippet_html,
        is_dm: matches!(kind, NotificationKind::Dm),
        reply_supported,
        reply_expires_hours: REPLY_TOKEN_TTL_HOURS,
        settings_url: &settings_url,
    };
    let text_view = NotificationText {
        sender_label: &sender_label,
        room_label: room_name,
        message_url: &message_url,
        snippet_plain: &snippet_plain,
        is_dm: matches!(kind, NotificationKind::Dm),
        reply_supported,
        reply_expires_hours: REPLY_TOKEN_TTL_HOURS,
        settings_url: &settings_url,
    };

    let html_body = match html_view.render() {
        Ok(b) => b,
        Err(e) => return DispatchOutcome::RenderFailed(format!("html: {e}")),
    };
    let text_body = match text_view.render() {
        Ok(b) => b,
        Err(e) => return DispatchOutcome::RenderFailed(format!("text: {e}")),
    };

    // 8. Submit.
    match mailer
        .send_multipart_with_reply_to(email, reply_to.as_deref(), &subject, &text_body, &html_body)
        .await
    {
        Ok(()) => DispatchOutcome::Sent,
        Err(e) => DispatchOutcome::SendFailed(format!("{e}")),
    }
}

/// Resolve the sender display name for the notification email. Mirrors
/// `routes::resolve_msg_author` but returns just the label (we don't need
/// the avatar etc here).
async fn resolve_sender_label(state: &AppState, raw: &db::chat::RawMessage) -> Option<String> {
    if let Some(wid) = raw.webhook_id {
        if let Ok(Some(w)) = db::webhooks::identity(&state.chat, wid).await {
            return Some(w.name);
        }
        return None;
    }
    if let Some(eid) = raw.email_inbox_id {
        if let Ok(Some(i)) = db::email_inbox::identity(&state.chat, eid).await {
            return Some(i.name);
        }
        return None;
    }
    if raw.user_id.is_empty() {
        return None;
    }
    match db::auth::find_user_by_id(&state.auth, &raw.user_id).await {
        Ok(Some(u)) => match u.display_name {
            Some(d) if !d.trim().is_empty() => Some(d),
            _ => Some(u.username),
        },
        _ => None,
    }
}

/// Read the ingress domain from the LC-77 IMAP config, if any. The
/// reply-to address is only meaningful when the operator has the
/// email-ingress feature configured AND enabled; we read the row and
/// extract `ingress_domain` (the same value the per-room inbox UI uses).
/// Returns None when no IMAP config row exists OR the secret key is
/// missing (the sealed creds can't be decrypted to extract the domain).
async fn ingress_domain(state: &AppState) -> Option<String> {
    let key = state.secret_key.as_ref()?;
    let cfg = db::imap_config::read(&state.settings, key.as_ref())
        .await
        .ok()
        .flatten()?;
    cfg.ingress_domain.filter(|d| !d.is_empty())
}

fn truncate_snippet(s: &str) -> String {
    if s.len() <= SNIPPET_MAX {
        return s.to_string();
    }
    let mut cut = SNIPPET_MAX;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = s[..cut].to_string();
    out.push('…');
    out
}
