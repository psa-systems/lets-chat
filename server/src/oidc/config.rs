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
