//! LC-77 address → inbox resolution.
//!
//! The IMAP-poll model's load-bearing uncertainty: the secret address
//! `<token>@<ingress-domain>` can land in any of several recipient
//! headers depending on how the operator routes mail into the polled
//! mailbox. We check, in order:
//!
//!   1. `Delivered-To` (set by the receiving MTA on actual delivery)
//!   2. `X-Original-To` (set by some MTAs when forwarding)
//!   3. `To`
//!   4. `Cc`
//!
//! First header whose local-part matches a known inbox secret wins. No
//! match → drop with `DropReason::AddressNoMatch`. Operator deployment
//! docs (commit 6) must pin this expectation; if a forwarder strips
//! these headers the feature breaks and the operator's only diagnostic
//! is the `tried_addresses` list in the drop log.

use mail_parser::{HeaderValue, Message};

use crate::db::email_inbox::{self, EmailInboxAuth};

/// Outcome of trying to resolve a polled message to an email-ingress inbox.
#[derive(Debug, Clone)]
pub enum ResolveOutcome {
    /// Active inbox; the caller posts the message.
    Match(EmailInboxAuth),
    /// Secret matched a known inbox but the inbox is revoked.
    /// The caller drops with `DropReason::RevokedInbox`.
    Revoked(EmailInboxAuth),
    /// No address in any of the four headers matched a known secret.
    /// `tried_addresses` is the comma-joinable list the drop log emits
    /// so an operator can see exactly which addresses the resolver saw.
    NotFound { tried_addresses: Vec<String> },
}

/// Header lookup order, exported as a constant so the docs and a
/// `tried_addresses` log can stay in lockstep with the code.
pub const HEADER_ORDER: &[&str] = &["Delivered-To", "X-Original-To", "To", "Cc"];

/// Resolve a polled message to an inbox row. `ingress_domain` is the
/// configured `<domain>` part of `<token>@<domain>`; addresses whose
/// domain (case-insensitive) does not match are skipped during scan so
/// stray recipient headers (e.g. a Bcc to a personal address that also
/// landed in this mailbox) cannot accidentally hit an inbox.
///
/// `secret_key` is the process-wide HMAC key used to hash the address's
/// local part into the form stored in `email_inboxes.secret_hash`.
/// Same keying as `crate::db::webhooks::find_by_secret_hash` callers use
/// (`crate::auth::hash_api_token(secret, token)`).
pub async fn resolve_inbox(
    pool: &sqlx::SqlitePool,
    secret_key: &[u8; 32],
    message: &Message<'_>,
    ingress_domain: &str,
) -> sqlx::Result<ResolveOutcome> {
    let mut tried: Vec<String> = Vec::new();
    let domain_lc = ingress_domain.to_ascii_lowercase();

    for header_name in HEADER_ORDER {
        for addr in addresses_from_header(message, header_name) {
            tried.push(addr.clone());
            let Some((local, domain)) = split_addr(&addr) else {
                continue;
            };
            if !domain.eq_ignore_ascii_case(&domain_lc) {
                continue;
            }
            let secret_hash = crate::auth::hash_api_token(secret_key, local);
            if let Some(row) = email_inbox::find_by_secret_hash(pool, &secret_hash).await? {
                if row.revoked_at.is_some() {
                    return Ok(ResolveOutcome::Revoked(row));
                }
                return Ok(ResolveOutcome::Match(row));
            }
        }
    }

    Ok(ResolveOutcome::NotFound {
        tried_addresses: tried,
    })
}

/// Extract every address string from headers matching `header_name`
/// (case-insensitive). Walks `message.headers()` rather than calling
/// the typed `Message::header(HeaderName)` accessor so an
/// operator-set custom header (`X-Original-To`, anything mail-parser
/// doesn't know about as a structured-address header) still surfaces.
fn addresses_from_header(message: &Message<'_>, header_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for h in message.headers() {
        if !h.name.as_str().eq_ignore_ascii_case(header_name) {
            continue;
        }
        push_from_value(&h.value, &mut out);
    }
    out
}

fn push_from_value(value: &HeaderValue<'_>, out: &mut Vec<String>) {
    match value {
        HeaderValue::Address(mail_parser::Address::List(list)) => {
            for a in list {
                if let Some(s) = a.address.as_deref() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                }
            }
        }
        HeaderValue::Address(mail_parser::Address::Group(groups)) => {
            for g in groups {
                for a in &g.addresses {
                    if let Some(s) = a.address.as_deref() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            out.push(trimmed.to_string());
                        }
                    }
                }
            }
        }
        HeaderValue::Text(t) => {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        HeaderValue::TextList(list) => {
            for t in list {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
        }
        _ => {}
    }
}

/// Split `local@domain` into `(local, domain)`. Returns `None` if there is
/// no `@` or either side is empty. We do not normalize the local part
/// (RFC 5321 says it's case-sensitive); the secret token alphabet is
/// case-preserved already by `auth::hash_api_token`.
fn split_addr(addr: &str) -> Option<(&str, &str)> {
    let (local, domain) = addr.rsplit_once('@')?;
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    Some((local, domain))
}
