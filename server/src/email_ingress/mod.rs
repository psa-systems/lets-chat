//! LC-77 email-to-room ingress. An operator-configured IMAP mailbox is
//! polled; each unseen message addressed to `<token>@<ingress-domain>`
//! posts to its room as a synthetic "Email" actor.
//!
//! Module map:
//!
//! - [`resolve`]: parsed-message → inbox row, via the
//!   Delivered-To / X-Original-To / To / Cc header precedence.
//! - [`parse`]: MIME body extraction + attachment-candidate scan. Takes
//!   the first text/plain part (or HTML-stripped fallback), prefixes
//!   the subject as Markdown-bold, and walks `message.attachments()` for
//!   up to `MAX_ATTACHMENTS_PER_MESSAGE` candidates.
//! - [`attachments`]: programmatic upload pipeline. Streams a raw
//!   attachment to a temp file, magic-byte sniffs via `infer::get_from_path`
//!   (NOT the sender's Content-Type), allowlist + EXIF-strip via the
//!   shared image pipeline, content-addressed storage. Per-attachment
//!   drops are non-fatal: the parent message still posts.
//! - [`actor`]: post a resolved + parsed message to its room as the
//!   email-inbox synthetic actor; broadcasts ChatEvent::NewMessage and
//!   links any successfully-processed attachment uploads.
//! - [`poll`]: the spawn_email_poll background task + the testable
//!   poll_once tick.
//!
//! Failure-log taxonomy (the SOLE diagnostic channel; no bounces, no
//! dead-letter folder in v1) is shared across the module via [`DropReason`].

pub mod actor;
pub mod attachments;
pub mod parse;
pub mod poll;
pub mod reply_actor;
pub mod resolve;

/// Reason a polled message was dropped without posting. Always logged at
/// WARN with the message-id, sender address, and a free-form detail string
/// so operators can diagnose "my email didn't post" support tickets from
/// logs alone. New variants require a docs/email-ingress.md update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// mail-parser returned an error or the message was empty/unparseable.
    ParseFail,
    /// No `<token>@<ingress-domain>` address matched a known inbox in any
    /// of Delivered-To / X-Original-To / To / Cc.
    AddressNoMatch,
    /// Secret matched but the inbox is revoked.
    RevokedInbox,
    /// Auto-Submitted / Precedence / X-Autoreply / List-Id heuristic
    /// flagged the message as machine-generated.
    LoopDetected,
    /// Per-inbox rate limit (60/min) tripped.
    RateLimited,
    /// `reply-<token>@<ingress-domain>` namespace; token row exists but
    /// `expires_at` is in the past. Distinguished from `AddressNoMatch`
    /// so an operator can tell a late reply ("user replied 2 weeks after
    /// the notification") from a garbage `reply-` address.
    ReplyExpired,
    /// Catch-all: DB error, disk full, anything that prevents posting.
    /// Detail must carry the specific cause for the log.
    InternalError,
}

impl DropReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ParseFail => "parse_fail",
            Self::AddressNoMatch => "address_no_match",
            Self::RevokedInbox => "revoked_inbox",
            Self::LoopDetected => "loop_detected",
            Self::RateLimited => "rate_limited",
            Self::ReplyExpired => "reply_expired",
            Self::InternalError => "internal_error",
        }
    }
}
