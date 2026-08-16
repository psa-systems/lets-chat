//! Integrity check for the desktop self-updater's downloaded artifact.
//!
//! The updater downloads a binary and `self_replace`s it into the running
//! process, so it hashes the bytes before writing anything: the manifest
//! carries a per-artifact `sha256` and `update::apply` refuses to install when
//! the download does not match it, or when the manifest names no hash at all.
//!
//! ## What this is worth (LC-709)
//!
//! The manifest used to carry a detached Ed25519 signature and the binary
//! embedded the public key. That was designed for a public distribution
//! channel; this one is membership-gated and authenticated, so the signature
//! was removed. With nothing signing the manifest, whoever can rewrite it can
//! rewrite the hash inside it too. The SHA-256 is therefore integrity against
//! a corrupt or truncated download and against a manifest that has drifted
//! from the binaries it names. It is not a control against an attacker who
//! controls the source; the authenticated fetch is what makes the source
//! trustworthy.

use sha2::{Digest, Sha256};

/// Verification failures. Distinct variants (not a bare string) so callers and
/// tests can assert the *reason* a binary was refused, mirroring the
/// property-not-proxy discipline of the LC-210 net_guard tests.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The manifest did not carry a sha256 for this platform's artifact.
    MissingArtifactHash,
    /// The artifact's sha256 (or the manifest's expected hex) was malformed.
    MalformedHash(String),
    /// Downloaded artifact bytes did not match the manifest's sha256.
    HashMismatch { expected: String, actual: String },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::MissingArtifactHash => {
                write!(f, "manifest has no sha256 for this platform's artifact")
            }
            VerifyError::MalformedHash(s) => write!(f, "malformed artifact sha256: {s}"),
            VerifyError::HashMismatch { expected, actual } => write!(
                f,
                "downloaded artifact sha256 {actual} does not match manifest {expected}"
            ),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Lowercase-hex SHA-256 of `bytes`, matching nushell's `hash sha256` output
/// (the form the release pipeline writes into the manifest). Test-only: the
/// production path hashes inside `verify_artifact_sha256`, and CI computes the
/// manifest hashes in nushell, so this helper exists for the fixtures only.
#[cfg(test)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

/// Confirm the downloaded artifact's SHA-256 matches the manifest's
/// `expected_hex`. Case-insensitive on the hex; compared as decoded bytes so a
/// differently cased but equal digest still matches.
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
