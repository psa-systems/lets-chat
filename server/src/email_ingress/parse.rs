//! LC-77 MIME → chat-body extraction. v1 is intentionally minimal:
//!
//! - Subject prefixes the body as a Markdown-bold first line.
//! - First text/plain part is the body. HTML-only messages fall through
//!   to mail-parser's best-effort text fallback; if neither exists, the
//!   body is empty.
//! - Hard cap at `MAX_BODY_BYTES` UTF-8 bytes, truncated with a marker
//!   so over-cap messages still post.
//!
//! Richer extraction (signature/quote stripping, attachments, HTML-only
//! fallback with explicit HTML-to-text conversion) is the LC-77 commit-5
//! surface. The v1 posture is "drop very little; widen on feedback."

use mail_parser::Message;

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
