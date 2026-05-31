//! LC-210-BINARY-INTEGRITY (#277): authenticity + integrity for the desktop
//! self-updater.
//!
//! LC-210 closed the SSRF / redirect-target vector on the update *fetch*, but
//! the downloaded artifact was still unverified: a redirect to a *public*
//! attacker host, a compromised mirror, or a TLS-trust break would serve a
//! binary that `update::apply` would `chmod +x` and `self_replace` into the
//! running process. The SSRF guard is necessary, not sufficient.
//!
//! ## Chain of trust
//!
//! 1. A 32-byte Ed25519 **public key** is embedded in the binary at build time
//!    (`PUBLIC_KEY_HEX`). The matching private key is a CI release secret and
//!    never ships.
//! 2. The release pipeline publishes `latest.json` (the manifest) AND a
//!    detached `latest.json.sig` (the raw 64-byte Ed25519 signature over the
//!    exact bytes of `latest.json`). The manifest carries a per-artifact
//!    `sha256`, so the hash is covered by the signature.
//! 3. The updater fetches both, verifies the signature over the raw manifest
//!    bytes **before parsing** (`verify_manifest_signature`), then downloads
//!    the platform binary and checks its SHA-256 against the signed manifest
//!    value (`verify_artifact_sha256`) **before** writing/replacing.
//!
//! ## Fail-closed
//!
//! If no public key is embedded (`PUBLIC_KEY_HEX` empty - the state before the
//! first signed release is cut), verification returns `NotConfigured` and the
//! updater refuses to apply anything. A build with no key can never install an
//! unverified binary; it simply has no working self-update.

use sha2::{Digest, Sha256};

/// Hex-encoded Ed25519 public key (64 hex chars = 32 bytes), injected at build
/// time. Set `LETS_CHAT_UPDATE_PUBLIC_KEY` in the release desktop build to the
/// hex public key whose private half signs `latest.json` in CI (see
/// `docs/desktop-update-signing.md`). Empty in unkeyed/dev builds, which makes
/// the updater fail closed (see module docs).
pub const PUBLIC_KEY_HEX: &str = match option_env!("LETS_CHAT_UPDATE_PUBLIC_KEY") {
    Some(k) => k,
    None => "",
};

/// Verification failures. Distinct variants (not a bare string) so callers and
/// tests can assert the *reason* a binary was refused, mirroring the
/// property-not-proxy discipline of the LC-210 net_guard tests.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// No public key embedded in this build; updates are disabled (fail-closed).
    NotConfigured,
    /// Embedded public key or the signature was not decodable / wrong length.
    MalformedKeyOrSig(String),
    /// Signature did not verify against the manifest bytes under the key.
    SignatureInvalid,
    /// The manifest did not carry a sha256 for this platform's artifact.
    MissingArtifactHash,
    /// The artifact's sha256 (or the manifest's expected hex) was malformed.
    MalformedHash(String),
    /// Downloaded artifact bytes did not match the signed sha256.
    HashMismatch { expected: String, actual: String },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::NotConfigured => write!(
                f,
                "update signing is not configured in this build; refusing to self-update"
            ),
            VerifyError::MalformedKeyOrSig(s) => {
                write!(f, "malformed update key or signature: {s}")
            }
            VerifyError::SignatureInvalid => {
                write!(f, "update manifest signature did not verify")
            }
            VerifyError::MissingArtifactHash => {
                write!(f, "manifest has no sha256 for this platform's artifact")
            }
            VerifyError::MalformedHash(s) => write!(f, "malformed artifact sha256: {s}"),
            VerifyError::HashMismatch { expected, actual } => write!(
                f,
                "downloaded artifact sha256 {actual} does not match signed {expected}"
            ),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verify a detached Ed25519 signature (raw 64 bytes) over `manifest_bytes`
/// using the build-embedded public key. The bytes verified are exactly the
/// bytes that were fetched, so this MUST be called before JSON parsing.
pub fn verify_manifest_signature(
    manifest_bytes: &[u8],
    signature: &[u8],
) -> Result<(), VerifyError> {
    verify_manifest_signature_with_key(manifest_bytes, signature, PUBLIC_KEY_HEX)
}

/// Same as [`verify_manifest_signature`] but with an explicit hex public key,
/// so tests can drive the full path with a generated keypair without depending
/// on the build-time const.
pub fn verify_manifest_signature_with_key(
    manifest_bytes: &[u8],
    signature: &[u8],
    public_key_hex: &str,
) -> Result<(), VerifyError> {
    use ed25519_dalek::{Signature, VerifyingKey};

    if public_key_hex.is_empty() {
        return Err(VerifyError::NotConfigured);
    }
    let key_bytes: [u8; 32] = hex::decode(public_key_hex)
        .map_err(|e| VerifyError::MalformedKeyOrSig(format!("public key hex: {e}")))?
        .try_into()
        .map_err(|_| VerifyError::MalformedKeyOrSig("public key must be 32 bytes".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| VerifyError::MalformedKeyOrSig(format!("public key: {e}")))?;
    let sig_bytes: [u8; 64] = signature
        .try_into()
        .map_err(|_| VerifyError::MalformedKeyOrSig("signature must be 64 bytes".into()))?;
    let signature = Signature::from_bytes(&sig_bytes);
    // verify_strict rejects malleable signatures and small-order public keys.
    verifying_key
        .verify_strict(manifest_bytes, &signature)
        .map_err(|_| VerifyError::SignatureInvalid)
}

/// Lowercase-hex SHA-256 of `bytes`, matching nushell's `hash sha256` output
/// (the form the release pipeline writes into the manifest). Test-only: the
/// production path hashes inside `verify_artifact_sha256`, and CI computes the
/// manifest hashes in nushell, so this helper exists for the fixtures only.
#[cfg(test)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

/// Confirm the downloaded artifact's SHA-256 matches the signed `expected_hex`.
/// Case-insensitive on the hex; compared as decoded bytes so a differently
/// cased but equal digest still matches.
pub fn verify_artifact_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), VerifyError> {
    let expected = hex::decode(expected_hex.trim())
        .map_err(|e| VerifyError::MalformedHash(format!("expected sha256 hex: {e}")))?;
    if expected.len() != 32 {
        return Err(VerifyError::MalformedHash(
            "expected sha256 must be 32 bytes".into(),
        ));
    }
    let actual = Sha256::digest(bytes);
    if actual.as_slice() == expected.as_slice() {
        Ok(())
    } else {
        Err(VerifyError::HashMismatch {
            expected: expected_hex.trim().to_ascii_lowercase(),
            actual: hex::encode(actual),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    // Deterministic keypair from a fixed 32-byte seed - no rng, so the test is
    // reproducible and needs no rand feature.
    fn test_keypair() -> (SigningKey, String) {
        let seed = [7u8; 32];
        let sk = SigningKey::from_bytes(&seed);
        let pk_hex = hex::encode(sk.verifying_key().to_bytes());
        (sk, pk_hex)
    }

    #[test]
    fn good_signature_verifies() {
        let (sk, pk_hex) = test_keypair();
        let manifest = br#"{"version":"v1.2.3","linux_x86_64":{"url":"https://x/","sha256":"00"}}"#;
        let sig = sk.sign(manifest).to_bytes();
        assert_eq!(
            verify_manifest_signature_with_key(manifest, &sig, &pk_hex),
            Ok(())
        );
    }

    #[test]
    fn tampered_manifest_fails() {
        let (sk, pk_hex) = test_keypair();
        let manifest = br#"{"version":"v1.2.3"}"#;
        let sig = sk.sign(manifest).to_bytes();
        // Flip one byte of the manifest AFTER signing - a redirect to a public
        // attacker host that serves a swapped manifest is exactly this case.
        let mut tampered = manifest.to_vec();
        tampered[2] ^= 0x01;
        assert_eq!(
            verify_manifest_signature_with_key(&tampered, &sig, &pk_hex),
            Err(VerifyError::SignatureInvalid)
        );
    }

    #[test]
    fn wrong_key_fails() {
        let (sk, _pk_hex) = test_keypair();
        let manifest = br#"{"version":"v1.2.3"}"#;
        let sig = sk.sign(manifest).to_bytes();
        // A different key (different seed) must not verify the signature.
        let other = hex::encode(
            SigningKey::from_bytes(&[9u8; 32])
                .verifying_key()
                .to_bytes(),
        );
        assert_eq!(
            verify_manifest_signature_with_key(manifest, &sig, &other),
            Err(VerifyError::SignatureInvalid)
        );
    }

    #[test]
    fn empty_embedded_key_is_not_configured() {
        // The real fail-closed default: an unkeyed build (PUBLIC_KEY_HEX == "")
        // refuses every manifest rather than trusting it.
        assert_eq!(PUBLIC_KEY_HEX, "", "test assumes an unkeyed build");
        let sig = [0u8; 64];
        assert_eq!(
            verify_manifest_signature(b"{}", &sig),
            Err(VerifyError::NotConfigured)
        );
    }

    #[test]
    fn malformed_key_and_sig_are_rejected_not_panicked() {
        let manifest = b"{}";
        let sig = [0u8; 64];
        // Odd-length / non-hex key.
        assert!(matches!(
            verify_manifest_signature_with_key(manifest, &sig, "zz"),
            Err(VerifyError::MalformedKeyOrSig(_))
        ));
        // Right hex but wrong length (16 bytes, not 32).
        assert!(matches!(
            verify_manifest_signature_with_key(manifest, &sig, &"ab".repeat(16)),
            Err(VerifyError::MalformedKeyOrSig(_))
        ));
        // Wrong-length signature.
        let (_sk, pk_hex) = test_keypair();
        assert!(matches!(
            verify_manifest_signature_with_key(manifest, &[0u8; 10], &pk_hex),
            Err(VerifyError::MalformedKeyOrSig(_))
        ));
    }

    #[test]
    fn artifact_hash_matches_and_mismatches() {
        let body = b"the real binary bytes";
        let good = sha256_hex(body);
        assert_eq!(verify_artifact_sha256(body, &good), Ok(()));
        // Uppercase hex still matches (decoded-byte compare).
        assert_eq!(
            verify_artifact_sha256(body, &good.to_ascii_uppercase()),
            Ok(())
        );
        // A swapped binary (same length even) must be refused.
        let swapped = b"the EVIL binary bytes";
        assert!(matches!(
            verify_artifact_sha256(swapped, &good),
            Err(VerifyError::HashMismatch { .. })
        ));
    }

    #[test]
    fn malformed_expected_hash_is_rejected() {
        assert!(matches!(
            verify_artifact_sha256(b"x", "nothex"),
            Err(VerifyError::MalformedHash(_))
        ));
        assert!(matches!(
            verify_artifact_sha256(b"x", "abcd"), // valid hex, wrong length
            Err(VerifyError::MalformedHash(_))
        ));
    }
}
