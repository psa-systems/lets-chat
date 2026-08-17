// Self-update support for lets-chat-desktop.
//
// LC-733: the release artifact is pulled from an OCI registry, authenticated as
// the user whose Bunyip login already granted them access to Let's Chat. The
// binaries are membership-gated and are not public, so the previous
// unauthenticated fetch of a Generic Packages URL could never have worked for a
// real user (it answers 401). The updater now:
//
//   1. resolves `{registry}/v2/{repository}/manifests/{tag}` for this platform,
//   2. reads the release version and the single artifact layer off the manifest,
//   3. downloads that one blob and checks it against the layer's SHA-256 digest
//      before `self_replace` swaps the running binary.
//
// The registry is configurable (`LETS_CHAT_UPDATE_REGISTRY_URL`) so an operator
// can mirror it; the shipped default is the membership-gated one. Nothing here
// is Bunyip-specific: the client speaks the OCI distribution API, and a registry
// that diverges from the spec is a registry-side problem.
//
// The credential is the registry token the server hands the webview for the
// signed-in user, forwarded to the native side by the inject.rs bridge and
// stored in the desktop config (see config.rs). `--update` is its own process,
// which is why the token goes through the config file rather than memory.
// `LETS_CHAT_UPDATE_TOKEN` overrides it for a build or a CI check that has no
// GUI session.
//
// LC-210: every fetch goes through `net_guard`, which validates each redirect
// hop against the public-IP filter (an injected redirect to an internal host is
// refused) and, since LC-733, strips the bearer on a cross-origin redirect.
// `LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE=1` exempts ONLY the initial URL from the
// IP filter, for an operator running a private internal registry mirror;
// redirect targets are still validated.
//
// LC-709: nothing signs the artifact. Distribution is membership-gated and
// authenticated, so the fetch itself is what makes the source trustworthy; the
// digest check catches a corrupt or truncated download and a manifest that has
// drifted from the blob it names. See `update_verify`.

use crate::config;
use crate::oci::{self, OciError, RegistryRef, RemoteArtifact};
use crate::update_verify;
use std::time::Duration;

/// Registry root the updater pulls from when the operator sets nothing.
///
/// LC-594 lesson, still applying: the compiled-in default must not drift from
/// where releases actually land, so it is injectable at build time rather than
/// only hardcoded.
///
/// The literal below is `dev.a8n.run`, the Forgejo host that serves the
/// membership-gated packages today and exposes an OCI endpoint under `/v2/`.
/// Bunyip's OCI proxy is the intended shipped default (it is what accepts a
/// Let's Chat user's token); its URL is supplied via
/// `LETS_CHAT_UPDATE_REGISTRY_URL` once that endpoint is published, per LC-733's
/// merge gate. An operator points anywhere else with the same env var.
pub const DEFAULT_REGISTRY_URL: &str = match option_env!("LETS_CHAT_UPDATE_REGISTRY_URL") {
    Some(u) if !u.is_empty() => u,
    _ => "https://dev.a8n.run",
};

/// Repository holding the desktop artifacts, `{owner}/{package}` matching what
/// `publish-release.yml` uploads to Generic Packages.
pub const DEFAULT_REPOSITORY: &str = match option_env!("LETS_CHAT_UPDATE_REPOSITORY") {
    Some(r) if !r.is_empty() => r,
    _ => "psa-systems-private/lets-chat",
};

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

// Upper bound on what we will buffer in memory for hash verification. The
// binary must be fully read to hash it before writing/replacing, so it is held
// in a Vec; the cap turns a hostile/oversized response into a clean error
// instead of an OOM. Generous vs a desktop binary (tens of MiB).
const MAX_ARTIFACT_BYTES: usize = 256 * 1024 * 1024;

/// Why an update check or install failed. Typed rather than a string so the
/// caller can tell an entitlement refusal from a transient network failure and
/// surface the two differently.
#[derive(Debug)]
pub enum UpdateError {
    UnsupportedPlatform,
    Registry(OciError),
    Verify(update_verify::VerifyError),
    Io(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::UnsupportedPlatform => {
                write!(f, "no release artifact is published for this platform/arch")
            }
            UpdateError::Registry(e) => write!(f, "{e}"),
            UpdateError::Verify(e) => write!(f, "verify downloaded artifact: {e}"),
            UpdateError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<OciError> for UpdateError {
    fn from(e: OciError) -> Self {
        UpdateError::Registry(e)
    }
}

impl UpdateError {
    /// True when the registry refused the caller's entitlement. The GUI raises
    /// this one, because unlike a transient outage it does not clear itself.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, UpdateError::Registry(e) if e.is_unauthorized())
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Tag holding this platform's artifact. `None` on a platform we publish no
/// binary for.
fn platform_tag() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("latest-windows-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("latest-linux-x86_64")
    } else {
        None
    }
}

/// Registry coordinate for this platform's artifact, after env overrides.
pub fn registry_ref() -> Result<RegistryRef, UpdateError> {
    let reference = env_non_empty("LETS_CHAT_UPDATE_TAG")
        .or_else(|| platform_tag().map(str::to_string))
        .ok_or(UpdateError::UnsupportedPlatform)?;
    Ok(RegistryRef {
        registry: env_non_empty("LETS_CHAT_UPDATE_REGISTRY_URL")
            .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string()),
        repository: env_non_empty("LETS_CHAT_UPDATE_REPOSITORY")
            .unwrap_or_else(|| DEFAULT_REPOSITORY.to_string()),
        reference,
    })
}

/// Bearer for the registry: the token the server minted for the signed-in user
/// (stored by the bridge), or an explicit override for a headless check.
fn registry_token() -> Option<String> {
    env_non_empty("LETS_CHAT_UPDATE_TOKEN").or_else(config::registry_token)
}

// LC-210: opt-out for the initial-URL public-IP filter only (redirect hops
// are always validated). Lets an operator point the updater at a private
// internal mirror without disabling redirect-target protection.
fn allow_private_initial() -> bool {
    matches!(
        std::env::var("LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE")
            .ok()
            .as_deref()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// Resolve the newest published artifact for this platform.
pub fn fetch_artifact() -> Result<RemoteArtifact, UpdateError> {
    let reference = registry_ref()?;
    let token = registry_token();
    Ok(oci::fetch_artifact(
        &reference,
        token.as_deref(),
        allow_private_initial(),
        MANIFEST_TIMEOUT,
    )?)
}

// Loose semver-ish comparison. Strips a leading `v`, takes the first three
// dot/`-`/`+` separated chunks, parses each as u64 (non-numeric chunks become
// 0), and compares lexicographically. Good enough to tell "v0.2.0" from
// "v0.1.0" and "0.2.0-1-gabc" from "0.2.0"; falls back to string inequality
// for anything more exotic.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(4)
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let r = parts(remote);
    let l = parts(local);
    if r == l {
        // Numeric parts identical; fall back to raw-string compare so a
        // dirty/post-tag build does not falsely advertise an update.
        remote.trim_start_matches('v') > local.trim_start_matches('v')
    } else {
        r > l
    }
}

pub fn local_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// Returns Some(remote_version) if an update is available, None if already on
// the latest, Err on network/authorization/parse failure.
pub fn check() -> Result<Option<String>, UpdateError> {
    let artifact = fetch_artifact()?;
    if is_newer(&artifact.version, local_version()) {
        Ok(Some(artifact.version))
    } else {
        Ok(None)
    }
}

pub enum ApplyOutcome {
    Updated(String),
    AlreadyLatest,
}

// Downloads the platform-appropriate binary and replaces the currently
// running executable in place. Returns AlreadyLatest (no-op) when the
// registry does not advertise a newer version so `--update` is idempotent.
pub fn apply() -> Result<ApplyOutcome, UpdateError> {
    let artifact = fetch_artifact()?;
    if !is_newer(&artifact.version, local_version()) {
        return Ok(ApplyOutcome::AlreadyLatest);
    }
    // The layer digest IS the expected hash; a descriptor without a sha256
    // digest never gets this far (oci::fetch_artifact refuses it).
    let expected_sha256 = artifact
        .sha256_hex()
        .ok_or(UpdateError::Verify(
            update_verify::VerifyError::MissingArtifactHash,
        ))?
        .to_string();

    // Download fully into memory so we can hash BEFORE writing anything to disk
    // or replacing the running binary.
    let body = oci::fetch_blob(
        &artifact,
        registry_token().as_deref(),
        allow_private_initial(),
        DOWNLOAD_TIMEOUT,
        MAX_ARTIFACT_BYTES,
    )?;
    // Length disagreement is caught before the hash so a truncated download
    // says so, instead of surfacing as an opaque digest mismatch.
    if artifact.size > 0 && body.len() as u64 != artifact.size {
        return Err(UpdateError::Io(format!(
            "downloaded {} bytes but the manifest describes {} for {}",
            body.len(),
            artifact.size,
            artifact.blob_url
        )));
    }
    update_verify::verify_artifact_sha256(&body, &expected_sha256).map_err(UpdateError::Verify)?;

    let mut tmp_path = std::env::temp_dir();
    tmp_path.push(format!(
        "lets-chat-desktop-update-{}-{}",
        std::process::id(),
        artifact.version,
    ));

    std::fs::write(&tmp_path, &body)
        .map_err(|e| UpdateError::Io(format!("write {}: {e}", tmp_path.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| UpdateError::Io(format!("chmod +x {}: {e}", tmp_path.display())))?;
    }

    self_replace::self_replace(&tmp_path)
        .map_err(|e| UpdateError::Io(format!("self-replace: {e}")))?;

    // Best-effort cleanup of the staging file; on Windows self-replace may
    // have already consumed it via rename, so failure here is not fatal.
    let _ = std::fs::remove_file(&tmp_path);

    Ok(ApplyOutcome::Updated(artifact.version))
}

// Fire-and-forget background check used at GUI startup. Matches all three arms
// of `check()` so no outcome can be silently dropped.
//
// LC-710: an update is announced through a native OS notification as well as
// stderr. A GUI launched from a desktop icon has no readable stderr, so the
// stderr line alone left the result invisible in the launch path that matters.
pub fn spawn_startup_check(app: tauri::AppHandle) {
    std::thread::spawn(move || match check() {
        Ok(Some(v)) => {
            let current = local_version();
            eprintln!(
                "lets-chat-desktop: update available: {v} (current: {current}). \
                 Run `lets-chat-desktop --update` to install."
            );
            notify_update_available(&app, &v, current);
        }
        // Already on the latest version: announcing that on every launch is
        // noise on both channels.
        Ok(None) => {}
        // LC-733: an entitlement refusal is not transient and does not clear
        // itself, so it is raised where the user is looking rather than left on
        // an stderr nobody reads. The check runs at startup, possibly before
        // the user has signed in, so the message says what to do about it.
        Err(e) if e.is_unauthorized() => {
            eprintln!("lets-chat-desktop: update check not authorized: {e}");
            notify(
                &app,
                "Let's Chat updates unavailable",
                "This copy could not prove it is entitled to Let's Chat updates. \
                 Sign in to Let's Chat in the app window, then try again.",
            );
        }
        Err(e) => {
            // Deliberate suppression of the USER-facing signal only: a startup
            // update check is best-effort, and a transient outage must not pop
            // a notification on every launch. The cause is still logged, and no
            // caller reads a failed check as "up to date" - this thread is the
            // only consumer of the result.
            eprintln!("lets-chat-desktop: update check failed: {e}");
        }
    });
}

// Native OS notification for an available update, via the notification plugin
// main.rs already registers.
fn notify_update_available(app: &tauri::AppHandle, available: &str, current: &str) {
    notify(
        app,
        "Let's Chat update available",
        &format!(
            "Version {available} is available (you are on {current}). \
             Run `lets-chat-desktop --update` to install."
        ),
    );
}

// Failing to post a notification is logged rather than dropped; the stderr
// line at each call site remains for terminal users.
fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("lets-chat-desktop: could not show notification ({title}): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LC-594: the default update source outlived the org it named. The repo
    /// moved off `a8n-tools` and the constant stayed behind, so a shipped
    /// binary would have polled an owner CI no longer publishes under - a dead
    /// self-updater, discoverable only by users, and unfixable for them
    /// precisely because self-update is what broke.
    ///
    /// Asserting the absence of the stale owner rather than a fixed URL keeps
    /// this honest under the build-time injection: a release build compiles in
    /// whatever registry CI names, so pinning the exact string would fail
    /// whenever CI is doing its job.
    #[test]
    fn default_registry_is_https_and_names_no_stale_org() {
        assert!(
            !DEFAULT_REGISTRY_URL.is_empty(),
            "an empty default leaves the updater with nowhere to poll"
        );
        assert!(
            !DEFAULT_REGISTRY_URL.contains("a8n-tools"),
            "default registry still points at the pre-transfer org: {DEFAULT_REGISTRY_URL}"
        );
        assert!(
            DEFAULT_REGISTRY_URL.starts_with("https://"),
            "the updater refuses plaintext sources: {DEFAULT_REGISTRY_URL}"
        );
        assert!(
            !DEFAULT_REPOSITORY.is_empty() && !DEFAULT_REPOSITORY.contains("a8n-tools"),
            "default repository is unusable: {DEFAULT_REPOSITORY}"
        );
    }

    #[test]
    fn platform_tag_is_published_for_supported_targets() {
        if cfg!(any(target_os = "linux", target_os = "windows")) && cfg!(target_arch = "x86_64") {
            assert!(platform_tag().is_some());
        }
    }

    #[test]
    fn version_comparison_still_holds() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
    }
}
