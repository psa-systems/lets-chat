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
//       "linux_x86_64":   { "url": "https://.../v0.2.0/lets-chat-desktop-linux-x86_64" },
//       "windows_x86_64": { "url": "https://.../v0.2.0/lets-chat-desktop-windows-x86_64.exe" }
//     }
//
// Forgejo's Generic Packages API serves files under
// /api/packages/{owner}/generic/{package}/{version}/{filename}, so the
// release workflow publishes the binaries under {version}/ and the manifest
// under /latest/latest.json. Pointing LETS_CHAT_UPDATE_URL at an alternative
// host (eg. for a fork, a staging mirror, or an offline mirror served over
// file://) overrides the default.

use serde::Deserialize;
use std::time::Duration;

pub const DEFAULT_UPDATE_URL: &str = "https://dev.a8n.run/api/packages/a8n-tools/generic/lets-chat";

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

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
}

fn update_url() -> String {
    std::env::var("LETS_CHAT_UPDATE_URL").unwrap_or_else(|_| DEFAULT_UPDATE_URL.to_string())
}

fn manifest_url(base: &str) -> String {
    format!("{}/latest/latest.json", base.trim_end_matches('/'))
}

pub fn fetch_manifest() -> Result<Manifest, String> {
    let url = manifest_url(&update_url());
    let response = ureq::get(&url)
        .timeout(MANIFEST_TIMEOUT)
        .call()
        .map_err(|e| format!("fetch manifest from {url}: {e}"))?;
    response
        .into_json::<Manifest>()
        .map_err(|e| format!("parse manifest JSON from {url}: {e}"))
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

    let mut tmp_path = std::env::temp_dir();
    tmp_path.push(format!(
        "lets-chat-desktop-update-{}-{}",
        std::process::id(),
        m.version,
    ));

    {
        let response = ureq::get(&artifact.url)
            .timeout(DOWNLOAD_TIMEOUT)
            .call()
            .map_err(|e| format!("download {}: {e}", artifact.url))?;
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("create {}: {e}", tmp_path.display()))?;
        std::io::copy(&mut response.into_reader(), &mut file)
            .map_err(|e| format!("write {}: {e}", tmp_path.display()))?;
    }

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

// Fire-and-forget background check used at GUI startup. Logs a single line to
// stderr if an update is available; silent otherwise (including on network
// failure, since we do not want a transient outage to noise up every launch).
pub fn spawn_startup_check() {
    std::thread::spawn(|| {
        if let Ok(Some(v)) = check() {
            eprintln!(
                "lets-chat-desktop: update available: {v} (current: {}). \
                 Run `lets-chat-desktop --update` to install.",
                local_version(),
            );
        }
    });
}
