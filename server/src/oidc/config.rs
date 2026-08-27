//! LC-22: env-derived Bunyip SSO configuration.
//!
//! Pure-RP cutover: the four `LETS_CHAT_BUNYIP_SSO_*` vars are mandatory at
//! startup. There is no `_ENABLED` flag; lets-chat refuses to start without
//! a working Bunyip OP per
//! `docs/lets-chat/sso/bunyip-only/00-overview.md` §what ships.

use url::Url;

#[derive(Debug, Clone)]
pub struct BunyipSsoConfig {
    pub issuer: Url,
    pub client_id: String,
    /// Plaintext client secret. Held as `String`; the `Debug` derive on the
    /// containing struct will print it - the only `Debug`-emitting code path is
    /// startup logging at INFO, and we don't log the config there. If a future
    /// log site needs to dump `AppState` we will switch to a redacting wrapper.
    pub client_secret: String,
    pub redirect_uri: Url,
}

impl BunyipSsoConfig {
    /// Returns `Ok(_)` only when every required env var parses. Missing or
    /// malformed values are an `Err`; startup turns that into a panic so the
    /// operator notices.
    pub fn from_env() -> Result<Self, String> {
        let issuer = required_var("LETS_CHAT_BUNYIP_SSO_ISSUER")?;
        let issuer = Url::parse(&issuer)
            .map_err(|e| format!("LETS_CHAT_BUNYIP_SSO_ISSUER is not a valid URL: {e}"))?;
        let client_id = required_var("LETS_CHAT_BUNYIP_SSO_CLIENT_ID")?;
        let client_secret = required_var("LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET")?;
        let redirect_uri = required_var("LETS_CHAT_BUNYIP_SSO_REDIRECT_URI")?;
        let redirect_uri = Url::parse(&redirect_uri)
            .map_err(|e| format!("LETS_CHAT_BUNYIP_SSO_REDIRECT_URI is not a valid URL: {e}"))?;
        Ok(Self {
            issuer,
            client_id,
            client_secret,
            redirect_uri,
        })
    }
}

fn required_var(name: &str) -> Result<String, String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(format!("{name} is required for Bunyip SSO startup")),
    }
}

/// LC-826: the development-only opt-out of the mandatory RP. When
/// `LETS_CHAT_DEV_NO_SSO` is `1` / `true` / `yes` (case-insensitive) the server
/// boots with no Bunyip client at all: nobody can sign in and the SSO routes
/// answer with a "not configured" login error. It exists so the local smoke
/// (`just verify` / `dev/server-up`) can boot without an identity provider;
/// main.rs logs a warning when it is set. Never set it on a real deployment.
pub fn dev_no_sso_opt_out() -> bool {
    dev_no_sso_value(std::env::var("LETS_CHAT_DEV_NO_SSO").ok().as_deref())
}

/// Pure half of [`dev_no_sso_opt_out`]: only an explicit affirmative counts,
/// so a stray empty or `0` value never disables sign-in.
fn dev_no_sso_value(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

#[cfg(test)]
mod tests {
    use super::dev_no_sso_value;

    #[test]
    fn dev_no_sso_needs_an_explicit_affirmative() {
        for on in ["1", "true", "TRUE", " yes "] {
            assert!(dev_no_sso_value(Some(on)), "{on:?}");
        }
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("maybe"),
        ] {
            assert!(!dev_no_sso_value(off), "{off:?}");
        }
    }
}
