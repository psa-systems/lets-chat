//! Email transport for the phase 22 digest feature and the admin
//! "send test email" button.
//!
//! Public surface: the `EmailClient` trait (one method, `send`) and an
//! `EmailMessage` value type. Implementations:
//!   * `LettreEmailClient` (production): wraps an async lettre transport
//!     configured from a loaded `SmtpConfig`. Connection is built per
//!     send because the use case is infrequent (hourly digest tick plus
//!     occasional admin test clicks), so connection pooling is not worth
//!     the lifecycle complexity.
//!   * `MockEmailClient` (test-only): records every send into a Mutex
//!     so tests can assert on what was sent without a real SMTP server.
//!     Built with `#[cfg(any(test, debug_assertions))]` so integration
//!     tests in `server/tests/` can instantiate it; the release binary
//!     never references it.

use std::sync::Arc;

use crate::db::smtp_settings::{SmtpConfig, TlsMode};

/// One email, ready to send. No attachments: the digest renders inline
/// plaintext + HTML, both `<= 100KB` per the design doc, no MIME beyond
/// `multipart/alternative`.
#[derive(Debug, Clone)]
pub struct EmailMessage {
    pub to: String,
    pub from: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("smtp transport error: {0}")]
    Transport(String),
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    #[error("message build error: {0}")]
    Build(String),
}

#[async_trait::async_trait]
pub trait EmailClient: Send + Sync {
    async fn send(&self, msg: EmailMessage) -> Result<(), EmailError>;
}

/// Production `EmailClient`. Holds the loaded `SmtpConfig` snapshot.
///
/// The snapshot is taken at construction time. If the admin changes SMTP
/// settings via the admin form, the in-memory snapshot does NOT refresh:
/// the operator restarts to pick up the new config. This matches the
/// VAPID-keypair model from phase 16 and keeps the construction path
/// simple. The admin "Send test email" route bypasses this snapshot and
/// constructs a fresh `LettreEmailClient` from the current DB row, so
/// the button always tests what is currently saved.
pub struct LettreEmailClient {
    pub config: SmtpConfig,
}

impl LettreEmailClient {
    pub fn new(config: SmtpConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl EmailClient for LettreEmailClient {
    async fn send(&self, msg: EmailMessage) -> Result<(), EmailError> {
        use lettre::message::{header::ContentType, MultiPart, SinglePart};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

        let from = msg
            .from
            .parse()
            .map_err(|e| EmailError::InvalidAddress(format!("from: {e}")))?;
        let to = msg
            .to
            .parse()
            .map_err(|e| EmailError::InvalidAddress(format!("to: {e}")))?;
        let email = Message::builder()
            .from(from)
            .to(to)
            .subject(&msg.subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(msg.text_body),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(msg.html_body),
                    ),
            )
            .map_err(|e| EmailError::Build(format!("{e}")))?;

        let mut builder: lettre::transport::smtp::AsyncSmtpTransportBuilder = match self
            .config
            .tls_mode
        {
            TlsMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.host)
                .map_err(|e| EmailError::Transport(format!("starttls relay: {e}")))?,
            TlsMode::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.host)
                .map_err(|e| EmailError::Transport(format!("tls relay: {e}")))?,
            TlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.config.host),
        };
        builder = builder.port(self.config.port);
        if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
            if !user.is_empty() && !pass.is_empty() {
                builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
            }
        }
        let transport = builder.build();
        transport
            .send(email)
            .await
            .map(|_| ())
            .map_err(|e| EmailError::Transport(format!("{e}")))
    }
}

/// Construct a one-off `EmailClient` from the currently-saved SMTP row.
/// Used by the admin "Send test email" handler so the operator can verify
/// config changes without a process restart.
///
/// Returns `Ok(None)` when SMTP is not configured (host empty) so the
/// caller can render a clear "not configured" message instead of trying
/// to send and failing with an opaque DNS error.
pub async fn build_from_current_config(
    settings_pool: &sqlx::SqlitePool,
    secret_key: &[u8; 32],
) -> Result<Option<Arc<dyn EmailClient>>, crate::error::AppError> {
    let cfg = crate::db::smtp_settings::load(settings_pool, secret_key).await?;
    match cfg {
        Some(c) if !c.host.is_empty() => Ok(Some(Arc::new(LettreEmailClient::new(c)))),
        _ => Ok(None),
    }
}

#[cfg(any(test, debug_assertions))]
mod mock {
    use super::*;
    use std::sync::Mutex;

    /// Test-only `EmailClient` that records every send. Live behind
    /// `#[cfg(any(test, debug_assertions))]` so release builds do not
    /// pull it in; integration tests in `server/tests/` use this in
    /// place of `LettreEmailClient` to assert on what was sent without
    /// touching a real SMTP server.
    pub struct MockEmailClient {
        pub sent: Mutex<Vec<EmailMessage>>,
        pub fail_next: Mutex<bool>,
    }

    impl Default for MockEmailClient {
        fn default() -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                fail_next: Mutex::new(false),
            }
        }
    }

    impl MockEmailClient {
        pub fn taken(&self) -> Vec<EmailMessage> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }
    }

    #[async_trait::async_trait]
    impl EmailClient for MockEmailClient {
        async fn send(&self, msg: EmailMessage) -> Result<(), EmailError> {
            let mut fail = self.fail_next.lock().unwrap();
            if *fail {
                *fail = false;
                return Err(EmailError::Transport("mock: forced failure".into()));
            }
            drop(fail);
            self.sent.lock().unwrap().push(msg);
            Ok(())
        }
    }
}

#[cfg(any(test, debug_assertions))]
pub use mock::MockEmailClient;
