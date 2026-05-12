use std::sync::Arc;

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

/// Mailer configured from a flat set of environment variables:
/// - `SMTP_HOST` (required) - relay hostname
/// - `SMTP_PORT` (required) - relay port as a decimal integer
/// - `SMTP_TLS` (required) - one of `tls` / `starttls` / `none`. Boolean
///   aliases (`true`/`yes`/`1` -> `tls`, `false`/`no`/`0` -> `none`) are
///   accepted so deployments that already model TLS as a flag don't have to
///   add a third state on day one.
/// - `SMTP_USERNAME`, `SMTP_PASSWORD` (optional, paired) - credentials.
///   When both are unset the transport opens unauthenticated.
/// - `SMTP_FROM` (required) - From address attached to outbound mail.
///
/// Any required var missing or empty disables outbound mail entirely; the
/// `Option<Mailer>` returned here is the same signal the `/forgot` and
/// `/reset/{token}` routes use to decide whether to return 404.
#[derive(Clone)]
pub struct Mailer {
    transport: Arc<AsyncSmtpTransport<Tokio1Executor>>,
    from: String,
}

impl Mailer {
    pub fn from_env() -> Option<Self> {
        let host = env_nonempty("SMTP_HOST")?;
        let port_str = env_nonempty("SMTP_PORT")?;
        let port: u16 = match port_str.parse() {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(port = %port_str, "SMTP_PORT is not a valid u16; mail disabled");
                return None;
            }
        };
        let tls_raw = env_nonempty("SMTP_TLS")?;
        let tls_mode = match parse_tls_mode(&tls_raw) {
            Some(m) => m,
            None => {
                tracing::warn!(value = %tls_raw, "SMTP_TLS must be tls/starttls/none; mail disabled");
                return None;
            }
        };
        let from = env_nonempty("SMTP_FROM")?;

        // Username/password are optional but must agree: setting one without
        // the other is almost always a deployment mistake, so refuse rather
        // than silently send unauthenticated mail.
        let username = env_nonempty("SMTP_USERNAME");
        let password = env_nonempty("SMTP_PASSWORD");
        let creds = match (username, password) {
            (Some(u), Some(p)) => Some(Credentials::new(u, p)),
            (None, None) => None,
            _ => {
                tracing::warn!(
                    "SMTP_USERNAME and SMTP_PASSWORD must be set together; mail disabled"
                );
                return None;
            }
        };

        match build_transport(&host, port, tls_mode, creds) {
            Ok(t) => Some(Self {
                transport: Arc::new(t),
                from,
            }),
            Err(e) => {
                tracing::warn!(error = %e, "failed to build SMTP transport; mail disabled");
                None
            }
        }
    }

    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), MailError> {
        let from = self
            .from
            .parse()
            .map_err(|e: lettre::address::AddressError| MailError::Address(e.to_string()))?;
        let to_addr = to
            .parse()
            .map_err(|e: lettre::address::AddressError| MailError::Address(e.to_string()))?;
        let message = Message::builder()
            .from(from)
            .to(to_addr)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| MailError::Build(e.to_string()))?;
        self.transport
            .send(message)
            .await
            .map_err(|e| MailError::Smtp(e.to_string()))?;
        Ok(())
    }

    /// Send a `multipart/alternative` message with both plaintext and HTML
    /// bodies. Used by the email digest where the snippet rendering wants
    /// HTML formatting (bolded mentions, anchor tags) while remaining
    /// readable in plaintext-only mail clients.
    pub async fn send_multipart(
        &self,
        to: &str,
        subject: &str,
        text_body: &str,
        html_body: &str,
    ) -> Result<(), MailError> {
        use lettre::message::{MultiPart, SinglePart};

        let from = self
            .from
            .parse()
            .map_err(|e: lettre::address::AddressError| MailError::Address(e.to_string()))?;
        let to_addr = to
            .parse()
            .map_err(|e: lettre::address::AddressError| MailError::Address(e.to_string()))?;
        let message = Message::builder()
            .from(from)
            .to(to_addr)
            .subject(subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text_body.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html_body.to_string()),
                    ),
            )
            .map_err(|e| MailError::Build(e.to_string()))?;
        self.transport
            .send(message)
            .await
            .map_err(|e| MailError::Smtp(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlsMode {
    /// Implicit TLS from the first byte (legacy "SMTPS", commonly port 465).
    Implicit,
    /// Cleartext handshake, then upgrade via STARTTLS (commonly port 587).
    StartTls,
    /// Cleartext throughout. Only suitable for localhost-only relays.
    None,
}

fn parse_tls_mode(raw: &str) -> Option<TlsMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "tls" | "ssl" | "implicit" | "true" | "yes" | "on" | "1" => Some(TlsMode::Implicit),
        "starttls" | "stls" | "auto" => Some(TlsMode::StartTls),
        "none" | "off" | "false" | "no" | "0" | "plain" | "cleartext" => Some(TlsMode::None),
        _ => None,
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    let v = std::env::var(name).ok()?;
    let v = v.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn build_transport(
    host: &str,
    port: u16,
    tls_mode: TlsMode,
    creds: Option<Credentials>,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let mut builder = match tls_mode {
        TlsMode::Implicit | TlsMode::StartTls => {
            let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).port(port);
            let params = TlsParameters::new(host.to_string()).map_err(|e| e.to_string())?;
            b = b.tls(match tls_mode {
                TlsMode::Implicit => Tls::Wrapper(params),
                TlsMode::StartTls => Tls::Required(params),
                TlsMode::None => unreachable!(),
            });
            b
        }
        TlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).port(port),
    };
    if let Some(c) = creds {
        builder = builder.credentials(c);
    }
    Ok(builder.build())
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("address error: {0}")]
    Address(String),
    #[error("build error: {0}")]
    Build(String),
    #[error("smtp error: {0}")]
    Smtp(String),
}
