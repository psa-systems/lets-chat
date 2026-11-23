//! LC-1219: ephemeral TURN credentials.
//!
//! Mesh WebRTC calls (and the SFU's own fallback) need a TURN relay so peers
//! behind symmetric NATs or UDP-restrictive firewalls can still connect when a
//! direct STUN-reflexive path fails. We integrate with coturn's shared-secret
//! scheme (`use-auth-secret`, a.k.a. the "TURN REST API"): instead of a static
//! long-term username/password that would sit in every served page and could be
//! harvested for open-relay abuse, the server mints a SHORT-LIVED credential per
//! request:
//!
//!   username   = "<expiry-unix>:<user-id>"
//!   credential = base64( HMAC-SHA1( shared-secret, username ) )
//!
//! coturn recomputes the same HMAC from its configured secret and accepts the
//! credential only until `expiry`. The browser receives it as an ordinary
//! `RTCIceServer` entry. The base STUN list (`LETS_CHAT_ICE_SERVERS`) is served
//! unchanged when no TURN is configured, so this is purely additive.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

/// Process-global TURN config, read once from the environment. Kept out of
/// `AppState` so the ~180 test state builders do not each have to carry a field;
/// tests never call `init`, so `configured` is `None` and the STUN-only list is
/// served unchanged.
static TURN: OnceLock<Option<TurnConfig>> = OnceLock::new();

/// Load the TURN config from the environment once, at startup. Idempotent:
/// a second call is ignored.
pub fn init() {
    let _ = TURN.set(TurnConfig::from_env());
}

/// The configured TURN relay, or `None` when TURN is off (or `init` never ran).
pub fn configured() -> Option<&'static TurnConfig> {
    TURN.get().and_then(|o| o.as_ref())
}

/// TURN relay configuration, loaded once at startup from the environment.
#[derive(Clone)]
pub struct TurnConfig {
    /// One or more `turn:`/`turns:` URLs handed to the browser verbatim.
    urls: Vec<String>,
    /// coturn's `static-auth-secret`, shared out of band. Never served.
    secret: Vec<u8>,
    /// How long a minted credential stays valid.
    ttl: Duration,
}

impl TurnConfig {
    /// Build from the environment, or `None` when TURN is not configured (no
    /// URLs or no secret). `LETS_CHAT_TURN_URLS` is comma-separated;
    /// `LETS_CHAT_TURN_SECRET` is the coturn shared secret;
    /// `LETS_CHAT_TURN_TTL_SECS` is the credential lifetime (default 3600).
    pub fn from_env() -> Option<Self> {
        let urls: Vec<String> = std::env::var("LETS_CHAT_TURN_URLS")
            .ok()?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if urls.is_empty() {
            return None;
        }
        let secret = std::env::var("LETS_CHAT_TURN_SECRET")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let ttl = std::env::var("LETS_CHAT_TURN_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(3600);
        Some(Self {
            urls,
            secret: secret.into_bytes(),
            ttl: Duration::from_secs(ttl),
        })
    }

    /// Mint a time-limited `(username, credential)` for `user_id` as of `now`,
    /// following coturn's shared-secret scheme. Separated from `ice_entry` so
    /// the HMAC is unit-testable against a fixed clock.
    pub fn credentials(&self, user_id: &str, now: SystemTime) -> (String, String) {
        let expiry = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(self.ttl.as_secs());
        let username = format!("{expiry}:{user_id}");
        // HMAC accepts a key of any length, so this never errors.
        let mut mac = HmacSha1::new_from_slice(&self.secret).expect("HMAC key length is unbounded");
        mac.update(username.as_bytes());
        let credential =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        (username, credential)
    }

    /// The `RTCIceServer` JSON object for this relay, with a fresh credential.
    fn ice_entry(&self, user_id: &str, now: SystemTime) -> serde_json::Value {
        let (username, credential) = self.credentials(user_id, now);
        serde_json::json!({
            "urls": self.urls,
            "username": username,
            "credential": credential,
        })
    }
}

/// Append the TURN relay (with a fresh per-user credential) to a base
/// ice-servers JSON array string. Returns `base` unchanged when TURN is not
/// configured or `base` is not a JSON array, so a misconfiguration can never
/// strip the STUN servers the call already relies on.
pub fn ice_servers_with_turn(base: &str, turn: Option<&TurnConfig>, user_id: &str) -> String {
    let Some(turn) = turn else {
        return base.to_string();
    };
    let mut arr: Vec<serde_json::Value> = match serde_json::from_str(base) {
        Ok(serde_json::Value::Array(a)) => a,
        _ => return base.to_string(),
    };
    arr.push(turn.ice_entry(user_id, SystemTime::now()));
    serde_json::to_string(&serde_json::Value::Array(arr)).unwrap_or_else(|_| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TurnConfig {
        TurnConfig {
            urls: vec!["turn:turn.example.com:3478?transport=udp".to_string()],
            secret: b"s3cr3t".to_vec(),
            ttl: Duration::from_secs(3600),
        }
    }

    #[test]
    fn credentials_follow_the_coturn_rest_scheme() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let (username, credential) = cfg().credentials("alice", now);
        // username = "<now + ttl>:<user-id>"
        assert_eq!(username, "1003600:alice");
        // credential = base64(HMAC-SHA1(secret, username)); fixed vector, so a
        // change to the algorithm (or the username shape) is caught here.
        let mut mac = HmacSha1::new_from_slice(b"s3cr3t").unwrap();
        mac.update(b"1003600:alice");
        let expected =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert_eq!(credential, expected);
        // base64 of a 20-byte SHA-1 digest is 28 chars (one '=' pad).
        assert_eq!(credential.len(), 28);
    }

    #[test]
    fn expiry_advances_with_the_ttl() {
        let now = UNIX_EPOCH + Duration::from_secs(50);
        let mut c = cfg();
        c.ttl = Duration::from_secs(120);
        let (username, _) = c.credentials("bob", now);
        assert_eq!(username, "170:bob");
    }

    #[test]
    fn no_turn_returns_the_base_list_unchanged() {
        let base = r#"[{"urls":"stun:stun.l.google.com:19302"}]"#;
        assert_eq!(ice_servers_with_turn(base, None, "alice"), base);
    }

    #[test]
    fn turn_is_appended_after_the_stun_servers() {
        let base = r#"[{"urls":"stun:stun.l.google.com:19302"}]"#;
        let out = ice_servers_with_turn(base, Some(&cfg()), "alice");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2, "STUN kept, TURN appended");
        assert_eq!(arr[0]["urls"], "stun:stun.l.google.com:19302");
        assert_eq!(
            arr[1]["urls"][0],
            "turn:turn.example.com:3478?transport=udp"
        );
        assert!(arr[1]["username"].as_str().unwrap().ends_with(":alice"));
        assert!(!arr[1]["credential"].as_str().unwrap().is_empty());
    }

    #[test]
    fn a_non_array_base_is_returned_unchanged() {
        // A broken LETS_CHAT_ICE_SERVERS must never drop the relay silently into
        // an object; we return it verbatim so the existing behaviour holds.
        let base = r#"{"oops":true}"#;
        assert_eq!(ice_servers_with_turn(base, Some(&cfg()), "alice"), base);
    }
}
