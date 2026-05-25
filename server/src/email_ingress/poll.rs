//! LC-77 IMAP poll loop. Connects to the operator-configured mailbox,
//! processes each UNSEEN message, and marks it \Seen unconditionally
//! (success OR fail) so a poison message can never trigger retry-forever.
//!
//! The loop driver and the per-message handler are split:
//!
//! - [`spawn_email_poll`] is the startup-gated background task.
//! - [`poll_once`] drives one tick end-to-end (connect, fetch, process,
//!   mark \Seen, disconnect). Returns a [`TickReport`] for the log line.
//! - [`process_polled_message`] handles a single raw RFC 822 payload.
//!   Pure async function (no IMAP, no TLS), so integration tests can
//!   feed it `.eml` fixtures directly without mocking a transport.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use mail_parser::Message;
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use crate::db;
use crate::db::imap_config::ImapConfig;
use crate::rate_limit::{Outcome as RlOutcome, RateLimitKind};
use crate::state::AppState;

use super::resolve::{resolve_inbox, ResolveOutcome};
use super::{actor, parse, DropReason};

/// Poll cadence. Matches the brainstorm decision (5 minutes; email is
/// not real-time). Skip-first-tick pattern mirrors the retention sweeper.
pub const POLL_INTERVAL_SECS: u64 = 300;

/// Per-inbox messages-per-minute cap. Matches LC-74
/// `WEBHOOK_RATE_PER_MIN`. Over-limit messages drop silently with
/// `DropReason::RateLimited`.
pub const POLL_RATE_LIMIT_PER_MIN: u32 = 60;

/// Hard cap on the raw RFC 822 bytes we will FETCH per message. Larger
/// than the parsed-body cap in `parse::MAX_BODY_BYTES` because the raw
/// message carries headers, MIME boundaries, base64 expansion, etc.
/// mail-parser does NOT impose a header-size limit (LC-77 spike A
/// finding); this cap is the upstream bound that keeps a 1 MiB-header
/// adversary from consuming server memory.
pub const MAX_RAW_MESSAGE_BYTES: usize = 5 * 1024 * 1024;

/// Result of one poll tick. The driver logs this when any of the
/// counters is non-zero; idle ticks stay quiet.
#[derive(Debug, Default)]
pub struct TickReport {
    pub fetched: usize,
    pub posted: usize,
    /// Drop counts by `DropReason::as_str()`. Stored as a map so a future
    /// addition to the taxonomy lands a new key without breaking the log
    /// schema. `tracing::info!(... dropped = ?report.dropped, ...)` emits
    /// a structured value.
    pub dropped: HashMap<&'static str, usize>,
}

impl TickReport {
    fn count_drop(&mut self, reason: DropReason) {
        *self.dropped.entry(reason.as_str()).or_insert(0) += 1;
    }
}

/// Outcome of processing one polled message. The driver translates this
/// into a structured WARN log + a `dropped` counter bump.
#[derive(Debug)]
pub enum ProcessOutcome {
    Posted { message_id: i64 },
    Dropped { reason: DropReason, detail: String },
}

/// Reasons the poll loop's outer driver might fail. Per-message failures
/// are isolated and surface as `ProcessOutcome::Dropped`; this enum
/// represents tick-level failures (TCP down, auth rejected, etc.).
#[derive(Debug, thiserror::Error)]
pub enum PollError {
    #[error("tcp connect: {0}")]
    Connect(std::io::Error),
    #[error("tls handshake: {0}")]
    Tls(std::io::Error),
    #[error("server name: {0}")]
    ServerName(String),
    #[error("imap: {0}")]
    Imap(#[from] async_imap::error::Error),
    #[error("login")]
    Login,
}

/// Spawn the IMAP poll loop. Gated by:
/// - `LETS_CHAT_SECRET_KEY` configured (no key → no sealed creds to read).
/// - A non-NULL `imap_inbox_config` row.
/// - `imap_inbox_config.enabled = 1` and `ingress_domain` set.
///
/// All three gate checks happen at startup, not per-tick: flipping
/// `enabled` requires a server restart, matching the
/// `LETS_CHAT_RETENTION_SWEEP_ENABLED` precedent.
pub fn spawn_email_poll(state: AppState) {
    let Some(secret_key) = state.secret_key.clone() else {
        tracing::info!(
            "email ingress disabled: LETS_CHAT_SECRET_KEY not set; sealed IMAP creds unreadable"
        );
        return;
    };
    tokio::spawn(async move {
        let config = match db::imap_config::read(&state.settings, secret_key.as_ref()).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                tracing::info!(
                    "email ingress disabled: imap_inbox_config row not configured; configure via the admin settings page and restart to enable",
                );
                return;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "email ingress disabled: imap_inbox_config read failed",
                );
                return;
            }
        };
        if !config.enabled {
            tracing::info!("email ingress disabled: imap_inbox_config.enabled = 0");
            return;
        }
        let Some(ref ingress_domain) = config.ingress_domain else {
            tracing::info!(
                "email ingress disabled: imap_inbox_config.ingress_domain unset; no token@domain to resolve",
            );
            return;
        };
        tracing::info!(
            host = %config.host,
            port = config.port,
            tls = config.tls,
            folder = %config.folder,
            domain = %ingress_domain,
            interval_secs = POLL_INTERVAL_SECS,
            "email ingress enabled",
        );
        let mut tick = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match poll_once(&state, secret_key.as_ref(), &config).await {
                Ok(report) if report.fetched > 0 || !report.dropped.is_empty() => {
                    tracing::info!(
                        fetched = report.fetched,
                        posted = report.posted,
                        dropped = ?report.dropped,
                        "email ingress tick complete",
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "email ingress tick failed");
                }
            }
        }
    });
}

/// Drive one full IMAP poll tick: TCP+TLS connect → LOGIN → SELECT →
/// SEARCH UNSEEN → for each UID: FETCH BODY[] → process → STORE +Seen
/// (ALWAYS). Returns counts for the log line.
///
/// `process_polled_message` is called for each fetched message; any
/// outcome (Posted or Dropped) is followed by an unconditional \Seen
/// store so a poison message never re-fetches next tick. The \Seen
/// store also runs if processing panics or errors at the application
/// level; only an IMAP-level error from the STORE itself escapes here,
/// and that surfaces as a tick-level Err the caller logs.
pub async fn poll_once(
    state: &AppState,
    secret_key: &[u8; 32],
    config: &ImapConfig,
) -> Result<TickReport, PollError> {
    let connector = tls_connector();
    let server_name = ServerName::try_from(config.host.clone())
        .map_err(|e| PollError::ServerName(e.to_string()))?;
    let tcp = tokio::net::TcpStream::connect((config.host.as_str(), config.port))
        .await
        .map_err(PollError::Connect)?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(PollError::Tls)?;
    let client = async_imap::Client::new(tls);
    let mut session = client
        .login(&config.username, &config.password)
        .await
        .map_err(|(_, _)| PollError::Login)?;

    session.select(&config.folder).await?;
    let unseen: Vec<u32> = session.uid_search("UNSEEN").await?.into_iter().collect();
    let mut report = TickReport::default();
    let ingress_domain = config
        .ingress_domain
        .as_deref()
        .expect("spawn_email_poll guards ingress_domain.is_some()");
    for uid in &unseen {
        report.fetched += 1;
        let raw = match fetch_one(&mut session, *uid).await {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(
                    target: "email_ingress::drop",
                    uid = *uid,
                    error = %e,
                    "FETCH failed; marking Seen and continuing",
                );
                report.count_drop(DropReason::InternalError);
                let _ = mark_seen(&mut session, *uid).await;
                continue;
            }
        };
        if raw.len() > MAX_RAW_MESSAGE_BYTES {
            tracing::warn!(
                target: "email_ingress::drop",
                uid = *uid,
                bytes = raw.len(),
                cap = MAX_RAW_MESSAGE_BYTES,
                reason = "internal_error",
                detail = "raw message over MAX_RAW_MESSAGE_BYTES cap",
                "email ingress dropped message",
            );
            report.count_drop(DropReason::InternalError);
            let _ = mark_seen(&mut session, *uid).await;
            continue;
        }
        match process_polled_message(state, secret_key, ingress_domain, &raw).await {
            ProcessOutcome::Posted { message_id } => {
                tracing::info!(
                    target: "email_ingress",
                    uid = *uid,
                    message_id,
                    "email ingress posted",
                );
                report.posted += 1;
            }
            ProcessOutcome::Dropped { reason, detail } => {
                tracing::warn!(
                    target: "email_ingress::drop",
                    uid = *uid,
                    reason = reason.as_str(),
                    detail = %detail,
                    "email ingress dropped message",
                );
                report.count_drop(reason);
            }
        }
        if let Err(e) = mark_seen(&mut session, *uid).await {
            // A mark-Seen failure is unusual (the FETCH succeeded so the
            // session is alive). Log and continue; the next tick will see
            // the UID UNSEEN again and reprocess (rare duplicate is
            // accepted at-least-once per the brainstorm).
            tracing::warn!(
                target: "email_ingress",
                uid = *uid,
                error = %e,
                "STORE +Seen failed; message may reprocess next tick",
            );
        }
    }
    if let Err(e) = session.logout().await {
        tracing::warn!(error = %e, "IMAP LOGOUT failed (connection will drop)");
    }
    Ok(report)
}

/// FETCH one UID's full RFC 822 body. Wraps the async-imap stream API
/// into a single `Vec<u8>` for the caller.
async fn fetch_one(
    session: &mut async_imap::Session<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    uid: u32,
) -> Result<Vec<u8>, async_imap::error::Error> {
    let fetches: Vec<_> = session
        .uid_fetch(uid.to_string(), "(BODY.PEEK[] UID)")
        .await?
        .try_collect()
        .await?;
    let fetch = fetches.first().ok_or(async_imap::error::Error::Bad(
        "UID FETCH returned no rows".into(),
    ))?;
    Ok(fetch.body().map(<[u8]>::to_vec).unwrap_or_default())
}

/// STORE +FLAGS (\Seen) on one UID.
async fn mark_seen(
    session: &mut async_imap::Session<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    uid: u32,
) -> Result<(), async_imap::error::Error> {
    let _: Vec<_> = session
        .uid_store(uid.to_string(), "+FLAGS (\\Seen)")
        .await?
        .try_collect()
        .await?;
    Ok(())
}

/// Process one polled message end-to-end (parse → loop detect → resolve →
/// rate limit → extract body → post). Pure async function: takes raw
/// RFC 822 bytes, no transport involved. Tests feed this directly.
pub async fn process_polled_message(
    state: &AppState,
    secret_key: &[u8; 32],
    ingress_domain: &str,
    raw: &[u8],
) -> ProcessOutcome {
    let parser = mail_parser::MessageParser::default();
    let message = match parser.parse(raw) {
        Some(m) => m,
        None => {
            return ProcessOutcome::Dropped {
                reason: DropReason::ParseFail,
                detail: "mail-parser returned None (empty or unparseable)".to_string(),
            };
        }
    };
    if let Some(reason_header) = detect_loop(&message) {
        return ProcessOutcome::Dropped {
            reason: DropReason::LoopDetected,
            detail: format!("{reason_header} present"),
        };
    }
    let inbox = match resolve_inbox(&state.chat, secret_key, &message, ingress_domain).await {
        Ok(ResolveOutcome::Match(inbox)) => inbox,
        Ok(ResolveOutcome::Revoked(inbox)) => {
            return ProcessOutcome::Dropped {
                reason: DropReason::RevokedInbox,
                detail: format!("inbox id {} revoked_at = {:?}", inbox.id, inbox.revoked_at),
            };
        }
        Ok(ResolveOutcome::NotFound { tried_addresses }) => {
            return ProcessOutcome::Dropped {
                reason: DropReason::AddressNoMatch,
                detail: format!("tried: {}", tried_addresses.join(", ")),
            };
        }
        Err(e) => {
            return ProcessOutcome::Dropped {
                reason: DropReason::InternalError,
                detail: format!("resolve_inbox: {e}"),
            };
        }
    };
    let key = inbox.id.to_string();
    if let RlOutcome::Deny { retry_after } =
        state
            .rate_limits
            .check(RateLimitKind::EmailInbox, &key, POLL_RATE_LIMIT_PER_MIN)
    {
        return ProcessOutcome::Dropped {
            reason: DropReason::RateLimited,
            detail: format!(
                "inbox {} exceeded {}/min cap (retry_after {}s)",
                inbox.id, POLL_RATE_LIMIT_PER_MIN, retry_after
            ),
        };
    }
    let extracted = parse::extract_body(&message);
    if extracted.body.trim().is_empty() {
        return ProcessOutcome::Dropped {
            reason: DropReason::ParseFail,
            detail: "empty subject and body after parse".to_string(),
        };
    }
    match actor::post_email_message(state, &inbox, &extracted.body).await {
        actor::PostOutcome::Posted { message_id } => ProcessOutcome::Posted { message_id },
        actor::PostOutcome::Dropped { reason, detail } => {
            ProcessOutcome::Dropped { reason, detail }
        }
    }
}

/// Loop-detection header heuristic. Returns the first matching header
/// name (for the drop log's `detail` field) or `None` if the message
/// looks like a real human-sent email. Conservative: drops on
/// `List-Id` too, including legitimate automated senders that set it.
/// That trade-off is documented in `docs/email-ingress.md` so an
/// operator who sees `reason=loop_detected detail="List-Id present"`
/// in the log can diagnose the cause without re-deriving the policy.
pub fn detect_loop(message: &Message<'_>) -> Option<&'static str> {
    for h in message.headers() {
        let name = h.name.as_str();
        if name.eq_ignore_ascii_case("Auto-Submitted") {
            if !matches_auto_submitted_no(&h.value) {
                return Some("Auto-Submitted");
            }
        } else if name.eq_ignore_ascii_case("Precedence") {
            if matches_bulk_precedence(&h.value) {
                return Some("Precedence");
            }
        } else if name.eq_ignore_ascii_case("X-Autoreply")
            || name.eq_ignore_ascii_case("X-Autorespond")
            || name.eq_ignore_ascii_case("List-Id")
        {
            return Some(match () {
                _ if name.eq_ignore_ascii_case("X-Autoreply") => "X-Autoreply",
                _ if name.eq_ignore_ascii_case("X-Autorespond") => "X-Autorespond",
                _ => "List-Id",
            });
        }
    }
    None
}

fn matches_auto_submitted_no(value: &mail_parser::HeaderValue<'_>) -> bool {
    // `Auto-Submitted: no` is the only value that should NOT trigger the
    // drop. Anything else (auto-replied, auto-generated, auto-notified)
    // is machine-generated.
    if let mail_parser::HeaderValue::Text(t) = value {
        return t.trim().eq_ignore_ascii_case("no");
    }
    false
}

fn matches_bulk_precedence(value: &mail_parser::HeaderValue<'_>) -> bool {
    if let mail_parser::HeaderValue::Text(t) = value {
        let trimmed = t.trim().to_ascii_lowercase();
        return matches!(trimmed.as_str(), "bulk" | "list" | "junk");
    }
    false
}

fn tls_connector() -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}
