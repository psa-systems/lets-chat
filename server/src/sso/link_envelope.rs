//! HMAC-stamped envelope for the link-required interstitial.
//!
//! When the callback finds an existing local user by email but can't
//! auto-link (either `email_verified=false` or the provider's
//! `auto_link_verified_email` flag is off), it renders a page asking
//! the user to confirm by typing their existing password. The
//! verified id_token claims that go into that page must survive the
//! round-trip without being modifiable by the user; we encode them in
//! a server-stamped envelope and verify the HMAC on the POST that
//! comes back.
//!
//! Shape: `{base64url(json_payload)}.{base64url(hmac_sha256_tag)}`.
//! Both halves use URL-safe base64 with no padding so the value is
//! safe in a form field. The HMAC key is `LETS_CHAT_SECRET_KEY`,
//! same as TOTP secrets + SMTP password + the SSO client secret.
//!
//! The envelope carries a `not_after` unix-seconds timestamp; the
//! verify path rejects expired envelopes even with a valid tag so a
//! captured envelope can't be replayed indefinitely. Default lifetime
//! matches the `sso_flows` TTL (10 minutes).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Default envelope lifetime. Matches `sso_flows.expires_at`.
pub const ENVELOPE_TTL_SECONDS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkPayload {
    pub provider_id: String,
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub return_to: String,
    /// Unix seconds after which the envelope no longer verifies. Set
    /// to `now() + ENVELOPE_TTL_SECONDS` at mint time.
    pub not_after: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error("envelope has the wrong shape (expected `payload.tag`)")]
    Shape,
    #[error("envelope payload is not valid base64url: {0}")]
    BadBase64(#[from] base64::DecodeError),
    #[error("envelope payload is not valid JSON: {0}")]
    BadJson(#[from] serde_json::Error),
    #[error("envelope HMAC tag does not match payload (envelope was tampered with)")]
    BadTag,
    #[error("envelope has expired")]
    Expired,
}

/// Mint an envelope for the link-required page.
pub fn mint(key: &[u8; 32], payload: &LinkPayload) -> String {
    let body = serde_json::to_vec(payload).expect("LinkPayload always serializes");
    let body_b64 = URL_SAFE_NO_PAD.encode(&body);
    let tag = compute_tag(key, body_b64.as_bytes());
    let tag_b64 = URL_SAFE_NO_PAD.encode(tag);
    format!("{body_b64}.{tag_b64}")
}

/// Verify + decode an envelope. Returns the payload on success.
/// Rejects:
///   - malformed shape / base64 / JSON
///   - tag that doesn't match the body
///   - `not_after` in the past
pub fn verify(key: &[u8; 32], envelope: &str, now_unix: i64) -> Result<LinkPayload, EnvelopeError> {
    let (body_b64, tag_b64) = envelope.split_once('.').ok_or(EnvelopeError::Shape)?;
    let expected = compute_tag(key, body_b64.as_bytes());
    let given = URL_SAFE_NO_PAD
        .decode(tag_b64)
        .map_err(EnvelopeError::BadBase64)?;
    // `Mac::verify_slice` is the constant-time comparator. Falling
    // back to `==` here would leak timing on the tag.
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any-length key");
    mac.update(body_b64.as_bytes());
    if mac.verify_slice(&given).is_err() {
        return Err(EnvelopeError::BadTag);
    }
    // Belt-and-braces: also recheck the freshly-computed tag matches
    // the decoded bytes. `verify_slice` already does this, but the
    // extra equality guard makes the local invariant obvious.
    debug_assert_eq!(expected.as_slice(), given.as_slice());

    let body = URL_SAFE_NO_PAD
        .decode(body_b64)
        .map_err(EnvelopeError::BadBase64)?;
    let payload: LinkPayload = serde_json::from_slice(&body)?;
    // `not_after` is "valid through this unix second". An envelope
    // verified at exactly `not_after` must still be accepted; only
    // strictly later requests are expired. The prior `<=` rejected an
    // envelope on the boundary second, costing one second of lifetime
    // on every interstitial.
    if payload.not_after < now_unix {
        return Err(EnvelopeError::Expired);
    }
    Ok(payload)
}

fn compute_tag(key: &[u8; 32], body: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any-length key");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    fn payload(not_after: i64) -> LinkPayload {
        LinkPayload {
            provider_id: "stub".into(),
            issuer: "https://idp/".into(),
            subject: "sub-1".into(),
            email: "alice@example.com".into(),
            return_to: "/rooms/general".into(),
            not_after,
        }
    }

    #[test]
    fn round_trip_preserves_fields() {
        let p = payload(2_000_000_000);
        let env = mint(&key(), &p);
        let decoded = verify(&key(), &env, 100).unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn malformed_shape_errors() {
        let err = verify(&key(), "no-dot-here", 0).unwrap_err();
        assert!(matches!(err, EnvelopeError::Shape));
    }

    #[test]
    fn tampered_body_errors() {
        let env = mint(&key(), &payload(2_000_000_000));
        let (_body, tag) = env.split_once('.').unwrap();
        let evil_body = URL_SAFE_NO_PAD.encode(br#"{"provider_id":"evil"}"#);
        let evil = format!("{evil_body}.{tag}");
        let err = verify(&key(), &evil, 0).unwrap_err();
        assert!(matches!(err, EnvelopeError::BadTag));
    }

    #[test]
    fn wrong_key_errors() {
        let env = mint(&key(), &payload(2_000_000_000));
        let mut other = key();
        other[0] ^= 0xff;
        let err = verify(&other, &env, 0).unwrap_err();
        assert!(matches!(err, EnvelopeError::BadTag));
    }

    #[test]
    fn expired_envelope_errors() {
        let env = mint(&key(), &payload(1000));
        let err = verify(&key(), &env, 2000).unwrap_err();
        assert!(matches!(err, EnvelopeError::Expired));
    }

    /// `not_after` is the last second the envelope is valid, not the
    /// first second it isn't. Verifying at `now == not_after` must
    /// succeed; only `now > not_after` should error.
    #[test]
    fn boundary_second_still_valid() {
        let env = mint(&key(), &payload(1_000));
        verify(&key(), &env, 1_000).expect("not_after second is still valid");
        let err = verify(&key(), &env, 1_001).unwrap_err();
        assert!(matches!(err, EnvelopeError::Expired));
    }
}
