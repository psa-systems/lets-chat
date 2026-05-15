//! `SsoConfig`: per-provider OIDC settings loaded from the environment.
//!
//! L2 ships the single-provider env-var loader. The future BYO-SSO
//! providers-file loader will land alongside this in a later phase;
//! both produce the same `SsoConfig` shape so downstream code doesn't
//! care which loader populated it.

use url::Url;

/// Slug under which v1's single configured provider is stored. The
/// /auth/sso/* routes use this as the implicit provider id; BYO-SSO
/// makes the provider id part of the URL.
pub const DEFAULT_PROVIDER_ID: &str = "default";

/// Per-provider OIDC settings. Cloneable; cheap to pass through state.
#[derive(Debug, Clone)]
pub struct SsoConfig {
    /// Discovery URL prefix; we'll fetch
    /// `{issuer}/.well-known/openid-configuration` at startup.
    pub issuer: Url,
    /// OAuth client_id registered on the IdP (UUID for mokosh-server).
    pub client_id: String,
    /// Confidential-client secret. Held as `String` here; future
    /// refactors may wrap in `secrecy::SecretString` once the surface
    /// touches more than one logger.
    pub client_secret: String,
    /// Redirect URI registered with the IdP. Derived from
    /// `LETS_CHAT_BASE_URL` + `/auth/sso/callback` unless explicitly
    /// overridden via `LETS_CHAT_SSO_REDIRECT_URI`.
    pub redirect_uri: Url,
    /// Whether the deployment allows first-time SSO sign-ins (no
    /// existing local user, no email match) to auto-provision a fresh
    /// user. Defaults `false` per docs/lets-chat/sso/02-design-decisions.md (3).
    pub autoprovision: bool,
    /// Whether the login page hides the username/password form when
    /// SSO is configured. UX-only; the form is still reachable via
    /// `/login?password=1`. Per doc 02 (9).
    pub password_hidden: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SsoConfigError {
    #[error("LETS_CHAT_SSO_ISSUER is set but not a valid URL: {0}")]
    BadIssuer(url::ParseError),
    #[error("LETS_CHAT_SSO_REDIRECT_URI is set but not a valid URL: {0}")]
    BadRedirect(url::ParseError),
    #[error("LETS_CHAT_BASE_URL ({0}) is not a valid base for deriving the SSO redirect URI: {1}")]
    BadBase(String, url::ParseError),
    #[error("LETS_CHAT_SSO_ISSUER is set but LETS_CHAT_SSO_CLIENT_ID is missing")]
    MissingClientId,
    #[error("LETS_CHAT_SSO_ISSUER is set but LETS_CHAT_SSO_CLIENT_SECRET is missing")]
    MissingClientSecret,
}

impl SsoConfig {
    /// Load from environment. Returns:
    /// - `Ok(None)` when SSO is not configured (the `LETS_CHAT_SSO_ISSUER`
    ///   sentinel env var is unset). The deployment runs password-only.
    /// - `Ok(Some(cfg))` when all required vars are present + valid.
    /// - `Err(...)` when `ISSUER` is set but the rest is incomplete or
    ///   malformed. Startup aborts in that case so the operator gets a
    ///   clear failure rather than a silently-disabled SSO surface.
    pub fn from_env() -> Result<Option<Self>, SsoConfigError> {
        let issuer_raw = match std::env::var("LETS_CHAT_SSO_ISSUER").ok() {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Ok(None),
        };
        let issuer = Url::parse(&issuer_raw).map_err(SsoConfigError::BadIssuer)?;

        let client_id = std::env::var("LETS_CHAT_SSO_CLIENT_ID")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or(SsoConfigError::MissingClientId)?;
        let client_secret = std::env::var("LETS_CHAT_SSO_CLIENT_SECRET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or(SsoConfigError::MissingClientSecret)?;

        let redirect_uri = match std::env::var("LETS_CHAT_SSO_REDIRECT_URI").ok() {
            Some(s) if !s.trim().is_empty() => {
                Url::parse(s.trim()).map_err(SsoConfigError::BadRedirect)?
            }
            _ => {
                // Derive from LETS_CHAT_BASE_URL + /auth/sso/callback.
                let base = std::env::var("LETS_CHAT_BASE_URL")
                    .ok()
                    .map(|s| s.trim().trim_end_matches('/').to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "http://localhost:8080".to_string());
                let derived = format!("{base}/auth/sso/callback");
                Url::parse(&derived).map_err(|e| SsoConfigError::BadBase(base, e))?
            }
        };

        let autoprovision = parse_bool_env("LETS_CHAT_SSO_AUTOPROVISION", false);
        let password_hidden = parse_bool_env("LETS_CHAT_SSO_PASSWORD_HIDDEN", false);

        Ok(Some(SsoConfig {
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            autoprovision,
            password_hidden,
        }))
    }
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The env-var-based loader is process-global state; serialize the
    // tests so concurrent runs don't read each other's vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let snapshots: Vec<(&str, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, prev) in snapshots {
            match prev {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn issuer_unset_returns_none() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", None),
                ("LETS_CHAT_SSO_CLIENT_ID", None),
                ("LETS_CHAT_SSO_CLIENT_SECRET", None),
            ],
            || {
                assert!(SsoConfig::from_env().unwrap().is_none());
            },
        );
    }

    #[test]
    fn issuer_blank_returns_none() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", Some("   ")),
                ("LETS_CHAT_SSO_CLIENT_ID", None),
                ("LETS_CHAT_SSO_CLIENT_SECRET", None),
            ],
            || {
                assert!(SsoConfig::from_env().unwrap().is_none());
            },
        );
    }

    #[test]
    fn complete_set_returns_some() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", Some("https://msp-api.example.com")),
                ("LETS_CHAT_SSO_CLIENT_ID", Some("the-uuid")),
                ("LETS_CHAT_SSO_CLIENT_SECRET", Some("the-secret")),
                ("LETS_CHAT_BASE_URL", Some("https://chat.example.com")),
                ("LETS_CHAT_SSO_REDIRECT_URI", None),
                ("LETS_CHAT_SSO_AUTOPROVISION", None),
                ("LETS_CHAT_SSO_PASSWORD_HIDDEN", None),
            ],
            || {
                let cfg = SsoConfig::from_env().unwrap().unwrap();
                assert_eq!(cfg.issuer.as_str(), "https://msp-api.example.com/");
                assert_eq!(cfg.client_id, "the-uuid");
                assert_eq!(cfg.client_secret, "the-secret");
                assert_eq!(
                    cfg.redirect_uri.as_str(),
                    "https://chat.example.com/auth/sso/callback"
                );
                assert!(!cfg.autoprovision);
                assert!(!cfg.password_hidden);
            },
        );
    }

    #[test]
    fn issuer_invalid_url_errors() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", Some("not a url")),
                ("LETS_CHAT_SSO_CLIENT_ID", Some("c")),
                ("LETS_CHAT_SSO_CLIENT_SECRET", Some("s")),
            ],
            || {
                assert!(matches!(
                    SsoConfig::from_env(),
                    Err(SsoConfigError::BadIssuer(_))
                ));
            },
        );
    }

    #[test]
    fn missing_client_id_errors() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", Some("https://x.example.com")),
                ("LETS_CHAT_SSO_CLIENT_ID", None),
                ("LETS_CHAT_SSO_CLIENT_SECRET", Some("s")),
            ],
            || {
                assert!(matches!(
                    SsoConfig::from_env(),
                    Err(SsoConfigError::MissingClientId)
                ));
            },
        );
    }

    #[test]
    fn missing_client_secret_errors() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", Some("https://x.example.com")),
                ("LETS_CHAT_SSO_CLIENT_ID", Some("c")),
                ("LETS_CHAT_SSO_CLIENT_SECRET", None),
            ],
            || {
                assert!(matches!(
                    SsoConfig::from_env(),
                    Err(SsoConfigError::MissingClientSecret)
                ));
            },
        );
    }

    #[test]
    fn explicit_redirect_uri_overrides_base_url() {
        with_env(
            &[
                ("LETS_CHAT_SSO_ISSUER", Some("https://x.example.com")),
                ("LETS_CHAT_SSO_CLIENT_ID", Some("c")),
                ("LETS_CHAT_SSO_CLIENT_SECRET", Some("s")),
                (
                    "LETS_CHAT_SSO_REDIRECT_URI",
                    Some("https://custom.example.com/cb"),
                ),
                ("LETS_CHAT_BASE_URL", Some("https://ignored.example.com")),
            ],
            || {
                let cfg = SsoConfig::from_env().unwrap().unwrap();
                assert_eq!(cfg.redirect_uri.as_str(), "https://custom.example.com/cb");
            },
        );
    }

    #[test]
    fn autoprovision_parses_truthy_values() {
        for val in ["1", "true", "True", "TRUE", "yes", "on"] {
            with_env(
                &[
                    ("LETS_CHAT_SSO_ISSUER", Some("https://x.example.com")),
                    ("LETS_CHAT_SSO_CLIENT_ID", Some("c")),
                    ("LETS_CHAT_SSO_CLIENT_SECRET", Some("s")),
                    ("LETS_CHAT_SSO_AUTOPROVISION", Some(val)),
                ],
                || {
                    let cfg = SsoConfig::from_env().unwrap().unwrap();
                    assert!(cfg.autoprovision, "expected truthy for {val:?}");
                },
            );
        }
    }
}
