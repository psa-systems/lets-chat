//! LC-77 MIME → chat-body + attachment extraction.
//!
//! Body:
//! - Subject prefixes the body as a Markdown-bold first line.
//! - First text/plain part is the body. HTML-only messages fall through
//!   to mail-parser's best-effort text fallback; if neither exists, the
//!   body is empty.
//! - Hard cap at `MAX_BODY_BYTES` UTF-8 bytes, truncated with a marker
//!   so over-cap messages still post.
//!
//! Attachments:
//! - Walk `message.attachments()` (mail-parser's iterator over parts the
//!   sender flagged with `Content-Disposition: attachment` plus any
//!   non-text MIME parts under `multipart/mixed`).
//! - Cap at `MAX_ATTACHMENTS_PER_MESSAGE`; over-cap parts are dropped
//!   and logged INFO by the caller, the message itself still posts.
//! - The caller (`email_ingress::attachments::process_attachment`) does
//!   the magic-byte sniff, allowlist check, and EXIF strip on images;
//!   this module just produces `RawAttachment` blobs.

use mail_parser::{Message, MessagePart, MimeHeaders};

/// 64 KiB UTF-8 cap for the combined subject + body. Larger than the
/// LC-74 webhook cap (16 KiB, `routes::webhooks::WEBHOOK_MAX_TEXT_BYTES`)
/// because email bodies legitimately run larger than chat-as-API
/// payloads, but well under "a forwarded mail thread dump" size.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// Marker appended when the body is truncated. Renders as italic in the
/// markdown pipeline so the truncation is visible in-chat.
pub const TRUNCATION_MARKER: &str = "\n\n_[truncated]_";

/// Extracted chat-message body. The poll loop assembles this and passes
/// it to `actor::post_email_message`.
#[derive(Debug, Clone)]
pub struct ExtractedBody {
    /// Final chat-message body, already truncated with marker if needed.
    /// May be empty if the source message had no subject and no extractable
    /// body; the caller drops with `DropReason::ParseFail` in that case.
    pub body: String,
    /// True when the body was truncated to fit `MAX_BODY_BYTES`. Logged for
    /// the operator's diagnostic visibility; does not affect the post.
    pub truncated: bool,
}

/// Compose a chat-message body from a polled email. The current
/// implementation prefixes the subject as a Markdown-bold first line and
/// uses the first text/plain part as the body. If no plain-text part
/// exists, mail-parser's `body_text(0)` returns a best-effort fallback
/// (which may be empty); we treat empty body + empty subject as the
/// caller's signal to drop.
///
/// Commit 5 will replace this with the full MIME walk (text/plain
/// preference, HTML-stripped fallback, attachment extraction). The v1
/// shape is just enough to post a deterministic, well-formed text body
/// for round-trip integration tests.
pub fn extract_body(message: &Message<'_>) -> ExtractedBody {
    let subject = message.subject().map(str::trim).filter(|s| !s.is_empty());
    let body_text = message
        .body_text(0)
        .map(|c| c.into_owned())
        .unwrap_or_default();

    let composed = match subject {
        Some(subj) => {
            // Subject becomes a Markdown-bold first line. Escape any pre-
            // existing `**` so a malicious subject cannot close the bold
            // and inject arbitrary leading markdown; cheap and sufficient.
            let escaped = subj.replace("**", r"\*\*");
            if body_text.is_empty() {
                format!("**{escaped}**")
            } else {
                format!("**{escaped}**\n\n{body_text}")
            }
        }
        None => body_text,
    };

    if composed.len() <= MAX_BODY_BYTES {
        return ExtractedBody {
            body: composed,
            truncated: false,
        };
    }
    let mut truncated = composed;
    let cap = MAX_BODY_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    // Truncate to a UTF-8 char boundary at or below `cap` so the resulting
    // String never breaks a multi-byte sequence.
    let split_at = floor_char_boundary(&truncated, cap);
    truncated.truncate(split_at);
    truncated.push_str(TRUNCATION_MARKER);
    ExtractedBody {
        body: truncated,
        truncated: true,
    }
}

/// Find the highest valid UTF-8 char boundary ≤ `n`. Equivalent to the
/// nightly `str::floor_char_boundary`; reimplemented because we target
/// stable Rust.
fn floor_char_boundary(s: &str, n: usize) -> usize {
    if n >= s.len() {
        return s.len();
    }
    let mut i = n;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Maximum attachments processed per polled message. Surplus parts are
/// dropped (the message body still posts). Matches the brainstorm's "4
/// per message" cap.
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 4;

/// One extracted attachment candidate. The caller MUST run the magic-byte
/// sniff + allowlist + EXIF strip before persisting; the
/// `claimed_content_type` here is from the sender's Content-Type header
/// and is not trusted.
#[derive(Debug, Clone)]
pub struct RawAttachment {
    /// Display filename. Sanitized to a basename only (path separators
    /// stripped) so it cannot escape the attachments directory if a
    /// caller naively interpolates it into a path. Empty when neither
    /// Content-Disposition nor Content-Type supplied a name.
    pub filename: String,
    /// Sender-claimed MIME. The pipeline only uses this to inform error
    /// messages; the actual MIME is sniffed via `infer::get_from_path`.
    pub claimed_content_type: String,
    /// Decoded part bytes (base64/quoted-printable already undone by
    /// mail-parser). Length-capped by the IMAP-fetch boundary upstream
    /// (`poll::MAX_RAW_MESSAGE_BYTES`); the per-attachment size check
    /// happens in the attachment pipeline.
    pub bytes: Vec<u8>,
}

/// Extract attachment candidates from a polled message. Returns up to
/// `MAX_ATTACHMENTS_PER_MESSAGE` items. Drops parts that have no decoded
/// bytes (the parser was unable to decode them); the poll loop's log
/// counts the dropped-on-extract count separately from the
/// dropped-on-allowlist count.
pub fn extract_attachments(message: &Message<'_>) -> Vec<RawAttachment> {
    let mut out = Vec::new();
    for part in message.attachments() {
        if out.len() >= MAX_ATTACHMENTS_PER_MESSAGE {
            break;
        }
        if let Some(att) = attachment_from_part(part) {
            out.push(att);
        }
    }
    out
}

fn attachment_from_part(part: &MessagePart<'_>) -> Option<RawAttachment> {
    let bytes = part.contents();
    if bytes.is_empty() {
        return None;
    }
    let claimed_content_type = part
        .content_type()
        .map(|ct| match ct.subtype() {
            Some(sub) => format!("{}/{sub}", ct.ctype()),
            None => ct.ctype().to_string(),
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let raw_name = part
        .attachment_name()
        .map(str::to_string)
        .unwrap_or_default();
    let filename = sanitize_filename(&raw_name);
    Some(RawAttachment {
        filename,
        claimed_content_type,
        bytes: bytes.to_vec(),
    })
}

/// Strip path separators and control characters from a sender-supplied
/// filename. The sanitized value is only used for display and the
/// `filename` column on the upload row; the on-disk storage path is
/// content-addressed and never echoes this value.
fn sanitize_filename(s: &str) -> String {
    let basename = s.rsplit(['/', '\\']).next().unwrap_or(s).trim().to_string();
    let cleaned: String = basename
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();
    if cleaned.is_empty() {
        "attachment".to_string()
    } else if cleaned.len() > 255 {
        cleaned.chars().take(255).collect()
    } else {
        cleaned
    }
}
