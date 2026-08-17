//! LC-733: the slice of the OCI distribution API the self-updater needs.
//!
//! Let's Chat binaries are membership-gated. The updater therefore pulls them
//! from an OCI registry (Bunyip, which proxies the Forgejo Generic Packages the
//! release workflow publishes) authenticated as the signed-in user, instead of
//! fetching an anonymous URL that answers 401.
//!
//! Two requests, no container machinery:
//!
//! 1. `GET /v2/{repository}/manifests/{reference}` - the manifest names the
//!    release version (the `org.opencontainers.image.version` annotation) and
//!    describes exactly one layer: the binary.
//! 2. `GET /v2/{repository}/blobs/{digest}` - that one layer.
//!
//! This is an *artifact* pull, not an image pull: there is no config blob to
//! interpret, no layer stack to assemble and no tar to extract. The layer
//! descriptor's `digest` is the SHA-256 the updater verifies the download
//! against before replacing the running binary.
//!
//! Every request goes through `net_guard`, so the per-hop public-IP filter
//! still applies, and the bearer is dropped on a cross-origin redirect (an
//! OCI registry routinely bounces a blob GET to a storage backend).

use crate::net_guard::{self, GuardError};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

/// Manifest media types we ask for. The Docker v2 type is included because a
/// registry synthesising a view over existing packages may still label it that
/// way; the fields this module reads are identical in both.
pub const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";
const BLOB_ACCEPT: &str = "application/octet-stream";

/// Where the release version is read from. Set on the manifest, or on the one
/// layer descriptor.
const VERSION_ANNOTATION: &str = "org.opencontainers.image.version";

/// Failures of an OCI fetch. Distinct variants (not a bare string) so the
/// caller can tell "you are not entitled to this download" from "the network
/// is down" and say so to the user.
#[derive(Debug)]
pub enum OciError {
    /// The registry refused the credential (or the absence of one).
    Unauthorized {
        url: String,
        status: u16,
    },
    NotFound {
        url: String,
    },
    Http {
        url: String,
        status: u16,
    },
    Transport {
        url: String,
        source: GuardError,
    },
    Malformed {
        url: String,
        detail: String,
    },
    TooLarge {
        url: String,
        max: usize,
    },
}

impl std::fmt::Display for OciError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OciError::Unauthorized { url, status } => write!(
                f,
                "not authorized to download Let's Chat updates from {url} (HTTP {status}). \
                 These binaries are membership-gated: sign in to Let's Chat in the app window \
                 so it can pick up a fresh registry credential, then try again. If it still \
                 fails, your account is not entitled to the desktop binaries."
            ),
            OciError::NotFound { url } => write!(
                f,
                "no release artifact published at {url} for this platform (HTTP 404)"
            ),
            OciError::Http { url, status } => write!(f, "{url}: HTTP {status}"),
            OciError::Transport { url, source } => write!(f, "{url}: {source}"),
            OciError::Malformed { url, detail } => write!(f, "{url}: {detail}"),
            OciError::TooLarge { url, max } => {
                write!(f, "{url}: response body exceeds {max} bytes")
            }
        }
    }
}

impl std::error::Error for OciError {}

impl OciError {
    /// True when the registry rejected the caller's entitlement rather than
    /// failing for a transient reason. Drives the distinct user-facing message.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, OciError::Unauthorized { .. })
    }
}

/// A `{registry}/v2/{repository}` coordinate plus the tag (or digest) to pull.
#[derive(Debug, Clone)]
pub struct RegistryRef {
    pub registry: String,
    pub repository: String,
    pub reference: String,
}

impl RegistryRef {
    pub fn manifest_url(&self) -> String {
        format!(
            "{}/v2/{}/manifests/{}",
            self.registry.trim_end_matches('/'),
            self.repository.trim_matches('/'),
            self.reference
        )
    }

    pub fn blob_url(&self, digest: &str) -> String {
        format!(
            "{}/v2/{}/blobs/{}",
            self.registry.trim_end_matches('/'),
            self.repository.trim_matches('/'),
            digest
        )
    }
}

/// The one artifact the manifest describes, resolved and ready to download.
#[derive(Debug, Clone)]
pub struct RemoteArtifact {
    /// Release version, e.g. `v0.2.0`.
    pub version: String,
    /// Full descriptor digest, e.g. `sha256:abcd...`.
    pub digest: String,
    pub size: u64,
    pub blob_url: String,
}

impl RemoteArtifact {
    /// Lowercase-hex SHA-256 from the descriptor digest, for the integrity
    /// check before `self_replace`. `None` for any other digest algorithm.
    pub fn sha256_hex(&self) -> Option<&str> {
        self.digest.strip_prefix("sha256:")
    }
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    layers: Vec<Descriptor>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Descriptor {
    digest: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

/// Manifests are small; cap tight so a hostile endpoint cannot stream
/// gigabytes into the parser.
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Fetch and interpret the manifest for `reference`.
pub fn fetch_artifact(
    reference: &RegistryRef,
    token: Option<&str>,
    allow_private_initial: bool,
    timeout: Duration,
) -> Result<RemoteArtifact, OciError> {
    let url = reference.manifest_url();
    let body = get(
        &url,
        token,
        MANIFEST_ACCEPT,
        allow_private_initial,
        timeout,
        MAX_MANIFEST_BYTES,
    )?;
    let manifest: Manifest = serde_json::from_slice(&body).map_err(|e| OciError::Malformed {
        url: url.clone(),
        detail: format!("parse manifest JSON: {e}"),
    })?;
    parse_artifact(reference, &url, manifest)
}

fn parse_artifact(
    reference: &RegistryRef,
    url: &str,
    manifest: Manifest,
) -> Result<RemoteArtifact, OciError> {
    let layer = match manifest.layers.len() {
        1 => &manifest.layers[0],
        n => {
            return Err(OciError::Malformed {
                url: url.to_string(),
                detail: format!("expected exactly one artifact layer, manifest describes {n}"),
            })
        }
    };
    let version = manifest
        .annotations
        .get(VERSION_ANNOTATION)
        .or_else(|| layer.annotations.get(VERSION_ANNOTATION))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| OciError::Malformed {
            url: url.to_string(),
            detail: format!("manifest carries no {VERSION_ANNOTATION} annotation"),
        })?;
    if layer.digest.strip_prefix("sha256:").is_none() {
        return Err(OciError::Malformed {
            url: url.to_string(),
            detail: format!("artifact digest is not sha256: {}", layer.digest),
        });
    }
    Ok(RemoteArtifact {
        version,
        blob_url: reference.blob_url(&layer.digest),
        digest: layer.digest.clone(),
        size: layer.size,
    })
}

/// Download the artifact blob. Held in memory so it can be hashed before
/// anything is written to disk; `max` bounds a hostile or runaway response.
pub fn fetch_blob(
    artifact: &RemoteArtifact,
    token: Option<&str>,
    allow_private_initial: bool,
    timeout: Duration,
    max: usize,
) -> Result<Vec<u8>, OciError> {
    get(
        &artifact.blob_url,
        token,
        BLOB_ACCEPT,
        allow_private_initial,
        timeout,
        max,
    )
}

fn get(
    url: &str,
    token: Option<&str>,
    accept: &str,
    allow_private_initial: bool,
    timeout: Duration,
    max: usize,
) -> Result<Vec<u8>, OciError> {
    let response = net_guard::guarded_get_with_auth(
        url,
        token,
        &[("Accept", accept)],
        allow_private_initial,
        timeout,
    )
    .map_err(|e| classify(url, e))?;
    read_body_capped(response, max).map_err(|detail| match detail {
        BodyError::TooLarge => OciError::TooLarge {
            url: url.to_string(),
            max,
        },
        BodyError::Io(detail) => OciError::Malformed {
            url: url.to_string(),
            detail,
        },
    })
}

fn classify(url: &str, e: GuardError) -> OciError {
    match e {
        GuardError::HttpStatus(status @ (401 | 403)) => OciError::Unauthorized {
            url: url.to_string(),
            status,
        },
        GuardError::HttpStatus(404) => OciError::NotFound {
            url: url.to_string(),
        },
        GuardError::HttpStatus(status) => OciError::Http {
            url: url.to_string(),
            status,
        },
        other => OciError::Transport {
            url: url.to_string(),
            source: other,
        },
    }
}

enum BodyError {
    TooLarge,
    Io(String),
}

// Reads one byte past the cap so "exactly max" is distinguishable from
// "too large".
fn read_body_capped(response: ureq::Response, max: usize) -> Result<Vec<u8>, BodyError> {
    let mut buf = Vec::new();
    let mut reader = response.into_reader().take((max as u64) + 1);
    reader
        .read_to_end(&mut buf)
        .map_err(|e| BodyError::Io(format!("read response body: {e}")))?;
    if buf.len() > max {
        return Err(BodyError::TooLarge);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ref() -> RegistryRef {
        RegistryRef {
            registry: "https://registry.example.com/".to_string(),
            repository: "psa-systems-private/lets-chat".to_string(),
            reference: "latest-linux-x86_64".to_string(),
        }
    }

    fn parse(json: &str) -> Result<RemoteArtifact, OciError> {
        let manifest: Manifest = serde_json::from_str(json).expect("fixture parses");
        parse_artifact(
            &test_ref(),
            "https://registry.example.com/manifest",
            manifest,
        )
    }

    #[test]
    fn urls_follow_the_distribution_spec() {
        let r = test_ref();
        assert_eq!(
            r.manifest_url(),
            "https://registry.example.com/v2/psa-systems-private/lets-chat/manifests/latest-linux-x86_64"
        );
        assert_eq!(
            r.blob_url("sha256:ff"),
            "https://registry.example.com/v2/psa-systems-private/lets-chat/blobs/sha256:ff"
        );
    }

    #[test]
    fn manifest_yields_version_digest_and_blob_url() {
        let artifact = parse(
            r#"{
              "schemaVersion": 2,
              "mediaType": "application/vnd.oci.image.manifest.v1+json",
              "annotations": { "org.opencontainers.image.version": "v0.3.0" },
              "config": { "mediaType": "application/vnd.oci.empty.v1+json", "digest": "sha256:00", "size": 2 },
              "layers": [
                {
                  "mediaType": "application/octet-stream",
                  "digest": "sha256:11aa",
                  "size": 4096,
                  "annotations": { "org.opencontainers.image.title": "lets-chat-desktop-linux-x86_64" }
                }
              ]
            }"#,
        )
        .expect("well-formed artifact manifest");
        assert_eq!(artifact.version, "v0.3.0");
        assert_eq!(artifact.sha256_hex(), Some("11aa"));
        assert_eq!(artifact.size, 4096);
        assert_eq!(
            artifact.blob_url,
            "https://registry.example.com/v2/psa-systems-private/lets-chat/blobs/sha256:11aa"
        );
    }

    #[test]
    fn version_annotation_on_the_layer_is_accepted() {
        let artifact = parse(
            r#"{"layers":[{"digest":"sha256:22","size":1,
                "annotations":{"org.opencontainers.image.version":"v1.2.3"}}]}"#,
        )
        .expect("layer-level version annotation");
        assert_eq!(artifact.version, "v1.2.3");
    }

    /// A manifest we cannot read a version out of must fail, not install
    /// something under a guessed version.
    #[test]
    fn manifest_without_a_version_is_refused() {
        let err = parse(r#"{"layers":[{"digest":"sha256:22","size":1}]}"#).unwrap_err();
        assert!(matches!(err, OciError::Malformed { .. }), "got {err:?}");
    }

    /// An image (many layers) is not an artifact; refuse rather than guess
    /// which layer is the binary.
    #[test]
    fn multi_layer_manifest_is_refused() {
        let err = parse(
            r#"{"annotations":{"org.opencontainers.image.version":"v1"},
                "layers":[{"digest":"sha256:1","size":1},{"digest":"sha256:2","size":1}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, OciError::Malformed { .. }), "got {err:?}");
    }

    #[test]
    fn non_sha256_digest_is_refused() {
        let err = parse(
            r#"{"annotations":{"org.opencontainers.image.version":"v1"},
                "layers":[{"digest":"sha512:22","size":1}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, OciError::Malformed { .. }), "got {err:?}");
    }

    /// A 401/403 must be classified as an entitlement problem and say so in
    /// words a user can act on - not surface as a generic transport failure.
    #[test]
    fn unauthorized_status_is_classified_and_readable() {
        for status in [401, 403] {
            let err = classify(
                "https://registry.example.com/v2/x/manifests/latest",
                GuardError::HttpStatus(status),
            );
            assert!(
                err.is_unauthorized(),
                "HTTP {status} should be an auth error"
            );
            let msg = err.to_string();
            assert!(msg.contains("not authorized"), "{msg}");
            assert!(msg.contains("sign in"), "{msg}");
        }
        let other = classify("https://x/y", GuardError::HttpStatus(500));
        assert!(!other.is_unauthorized());
        assert!(!classify("https://x/y", GuardError::HttpStatus(404)).is_unauthorized());
    }
}
