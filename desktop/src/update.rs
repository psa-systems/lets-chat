// Self-update support for lets-chat-desktop.
//
// Pulls a small JSON manifest from {LETS_CHAT_UPDATE_URL}/latest/latest.json
// describing the newest released version and per-platform download URLs,
// compares the manifest's version to the compile-time CARGO_PKG_VERSION, and
// (on `--update`) downloads the matching binary and atomically swaps it in
// place via the self-replace crate.
//
// Manifest shape:
//     {
//       "version": "v0.2.0",
//       "linux_x86_64":   { "url": "https://.../v0.2.0/lets-chat-desktop-linux-x86_64",     "sha256": "<hex>" },
//       "windows_x86_64": { "url": "https://.../v0.2.0/lets-chat-desktop-windows-x86_64.exe", "sha256": "<hex>" }
//     }
//
// Forgejo's Generic Packages API serves files under
// /api/packages/{owner}/generic/{package}/{version}/{filename}, so the
// release workflow publishes the binaries under {version}/ and the manifest
// under /latest/latest.json. Pointing LETS_CHAT_UPDATE_URL at an alternative
// http(s) host (eg. a fork or a staging mirror) overrides the default.
//
// LC-210: every fetch here (manifest + binary download) goes through
// `net_guard::guarded_get`, which validates each redirect hop against the
// public-IP filter (an injected redirect to an internal host is refused).
// `LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE=1` exempts ONLY the initial URL from
// that filter, for an operator running a private internal update mirror;
// redirect targets are still validated.
//
// LC-709: the manifest is no longer signed. Distribution is membership-gated
// and authenticated, so the fetch itself is what makes the source trustworthy.
// The one remaining check is `apply` hashing the downloaded binary and
// comparing it to the manifest's `sha256` before `self_replace`, which catches
// a corrupt or truncated download and a manifest that has drifted from the
// binaries it names; a manifest with no hash for this platform is refused
// rather than installed. See `update_verify`.

use crate::net_guard;
use crate::update_verify;
use serde::Deserialize;
use std::io::Read;
use std::time::Duration;

/// Generic Packages root the updater reads when the operator sets no
/// `LETS_CHAT_UPDATE_URL`.
///
/// LC-594: this was hardcoded to the `a8n-tools` path and stayed there when the
/// repo moved orgs, so a shipped binary would have looked for its updates under
/// an owner CI no longer publishes to. The publish workflow now injects this
/// from the same `PACKAGE_OWNER` variable it uploads with, so the compiled-in
/// default cannot drift from the upload destination again. The literal below is
/// the fallback for builds that inject nothing (local and non-release builds).
///
/// The owner is `psa-systems-private`, not `psa-systems`: the org variable
/// `PSA_SYSTEMS_PRIVATE_PACKAGE_OWNER` resolves to the former, which is where
/// the `lets-chat` generic package actually lives. Read from the org rather than
/// inferred from the repo path, which names the latter.
///
/// Note that owner is a *private* org: an anonymous fetch of this URL is 401,
/// so this default cannot serve public users as-is. Authenticated distribution
/// is tracked separately (LC-733).
pub const DEFAULT_UPDATE_URL: &str = match option_env!("LETS_CHAT_UPDATE_BASE_URL") {
    Some(u) if !u.is_empty() => u,
    _ => "https://dev.a8n.run/api/packages/psa-systems-private/generic/lets-chat",
};

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

// Upper bound on what we will buffer in memory for hash verification. The
// binary must be fully read to hash it before writing/replacing, so it is held
// in a Vec; the cap turns a hostile/oversized response into a clean error
// instead of an OOM. Generous vs a desktop binary (tens of MiB).
const MAX_ARTIFACT_BYTES: usize = 256 * 1024 * 1024;
// The manifest is tiny; cap small so a hostile endpoint cannot stream
// gigabytes into the parser.
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub linux_x86_64: Option<Artifact>,
    #[serde(default)]
    pub windows_x86_64: Option<Artifact>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Artifact {
    pub url: String,
    // Lowercase-hex SHA-256 of the artifact. Optional in the type so a
    // malformed/legacy manifest deserializes; `apply` treats a missing hash as
    // a hard error (MissingArtifactHash) rather than installing unverified
    // bytes.
    #[serde(default)]
    pub sha256: Option<String>,
}

fn update_url() -> String {
    std::env::var("LETS_CHAT_UPDATE_URL").unwrap_or_else(|_| DEFAULT_UPDATE_URL.to_string())
}

// LC-210: opt-out for the initial-URL public-IP filter only (redirect hops
// are always validated). Lets an operator point LETS_CHAT_UPDATE_URL at a
// private internal mirror without disabling redirect-target protection.
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

fn manifest_url(base: &str) -> String {
    format!("{}/latest/latest.json", base.trim_end_matches('/'))
}

// Read a guarded response body into memory, refusing anything past `max` so a
// hostile endpoint cannot OOM us. Reads one byte past the cap to distinguish
// "exactly max" from "too large".
fn read_body_capped(response: ureq::Response, max: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut reader = response.into_reader().take((max as u64) + 1);
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response body: {e}"))?;
    if buf.len() > max {
        return Err(format!("response body exceeds {max} bytes"));
    }
    Ok(buf)
}

// Fetches `latest.json` and parses it. LC-709 removed the manifest signature,
// so there is nothing to verify here; the artifact hash check in `apply` is the
// remaining integrity gate.
pub fn fetch_manifest() -> Result<Manifest, String> {
    let base = update_url();
    let allow_private = allow_private_initial();

    let m_url = manifest_url(&base);
    let manifest_resp = net_guard::guarded_get(&m_url, allow_private, MANIFEST_TIMEOUT)
        .map_err(|e| format!("fetch manifest from {m_url}: {e}"))?;
    let manifest_bytes = read_body_capped(manifest_resp, MAX_MANIFEST_BYTES)?;

    serde_json::from_slice::<Manifest>(&manifest_bytes)
        .map_err(|e| format!("parse manifest JSON from {m_url}: {e}"))
}

pub fn platform_artifact(m: &Manifest) -> Option<&Artifact> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        m.windows_x86_64.as_ref()
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        m.linux_x86_64.as_ref()
    } else {
        None
    }
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
// the latest, Err on network/parse failure.
pub fn check() -> Result<Option<String>, String> {
    let m = fetch_manifest()?;
    if is_newer(&m.version, local_version()) {
        Ok(Some(m.version))
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
// manifest does not advertise a newer version so `--update` is idempotent.
pub fn apply() -> Result<ApplyOutcome, String> {
    let m = fetch_manifest()?;
    if !is_newer(&m.version, local_version()) {
        return Ok(ApplyOutcome::AlreadyLatest);
    }
    let artifact = platform_artifact(&m)
        .ok_or_else(|| "no release artifact published for this platform/arch".to_string())?;
    // A manifest missing the hash for our platform is refused rather than
    // installed unverified.
    let expected_sha256 = artifact
        .sha256
        .as_deref()
        .ok_or_else(|| update_verify::VerifyError::MissingArtifactHash.to_string())?;

    // Download fully into memory so we can hash BEFORE writing anything to disk
    // or replacing the running binary.
    let response = net_guard::guarded_get(&artifact.url, allow_private_initial(), DOWNLOAD_TIMEOUT)
        .map_err(|e| format!("download {}: {e}", artifact.url))?;
    let body = read_body_capped(response, MAX_ARTIFACT_BYTES)
        .map_err(|e| format!("download {}: {e}", artifact.url))?;
    update_verify::verify_artifact_sha256(&body, expected_sha256)
        .map_err(|e| format!("verify {}: {e}", artifact.url))?;

    let mut tmp_path = std::env::temp_dir();
    tmp_path.push(format!(
        "lets-chat-desktop-update-{}-{}",
        std::process::id(),
        m.version,
    ));

    std::fs::write(&tmp_path, &body).map_err(|e| format!("write {}: {e}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod +x {}: {e}", tmp_path.display()))?;
    }

    self_replace::self_replace(&tmp_path).map_err(|e| format!("self-replace: {e}"))?;

    // Best-effort cleanup of the staging file; on Windows self-replace may
    // have already consumed it via rename, so failure here is not fatal.
    let _ = std::fs::remove_file(&tmp_path);

    Ok(ApplyOutcome::Updated(m.version))
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
// main.rs already registers. Failing to post it is logged rather than dropped;
// the stderr line above remains for terminal users.
fn notify_update_available(app: &tauri::AppHandle, available: &str, current: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app
        .notification()
        .builder()
        .title("Let's Chat update available")
        .body(format!(
            "Version {available} is available (you are on {current}). \
             Run `lets-chat-desktop --update` to install."
        ))
        .show()
    {
        eprintln!("lets-chat-desktop: could not show update notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LC-594: the default update source outlived the org it named. The repo
    /// moved off `a8n-tools` and this constant stayed behind, so a shipped
    /// binary would have polled an owner CI no longer publishes under - a dead
    /// self-updater, discoverable only by users, and unfixable for them
    /// precisely because self-update is what broke.
    ///
    /// Asserting the absence of the stale owner rather than a fixed URL keeps
    /// this honest under the build-time injection: a release build compiles in
    /// the path derived from the publish workflow's PACKAGE_OWNER, so pinning
    /// the exact string would fail whenever CI is doing its job.
    #[test]
    fn default_update_url_does_not_name_a_stale_org() {
        assert!(
            !DEFAULT_UPDATE_URL.is_empty(),
            "an empty default leaves the updater with nowhere to poll"
        );
        assert!(
            !DEFAULT_UPDATE_URL.contains("a8n-tools"),
            "default update URL still points at the pre-transfer org: {DEFAULT_UPDATE_URL}"
        );
        assert!(
            DEFAULT_UPDATE_URL.starts_with("https://"),
            "the updater refuses plaintext sources: {DEFAULT_UPDATE_URL}"
        );
    }
}
