// Persistent on-disk config for the desktop wrapper.
//
// Stores the user's chosen server URL so the GUI URL editor on the welcome
// page survives restarts without requiring the user to export
// LETS_CHAT_SERVER_URL every time. The env var stays a higher-priority dev
// override; the config file is the durable default new users land on.
//
// LC-733: also stores the registry token the server hands the webview after a
// Bunyip login (see inject.rs). `--update` runs as its own process, so the
// token has to reach it through the file rather than through memory.
//
// Format: { "server_url": "https://chat.example.com", "registry_token": "..." }
//
// Location: dirs::config_dir().join("lets-chat-desktop/config.json") which
// resolves to $XDG_CONFIG_HOME/lets-chat-desktop on Linux,
// ~/Library/Application Support/lets-chat-desktop on macOS, and
// %APPDATA%\lets-chat-desktop on Windows.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// Bearer token for the update registry, minted for the signed-in user by
    /// the server route the bridge calls. Absent until the user has signed in
    /// at least once in this app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_token: Option<String>,
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lets-chat-desktop").join("config.json"))
}

pub fn load() -> Config {
    let Some(path) = config_path() else {
        eprintln!("lets-chat-desktop: no config dir on this platform; using defaults");
        return Config::default();
    };
    load_at(&path)
}

// LC-733: a read or parse failure is logged with its cause before falling back.
// Once the registry token lives in this file, a corrupt config would otherwise
// be indistinguishable from "not signed in" and the user would be told the
// wrong thing. A missing file stays silent: that is the normal first run.
fn load_at(path: &Path) -> Config {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!(
                "lets-chat-desktop: could not read config {}: {e}; using defaults",
                path.display()
            );
            return Config::default();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "lets-chat-desktop: could not parse config {}: {e}; using defaults",
                path.display()
            );
            Config::default()
        }
    }
}

/// Persist the server URL, preserving every other field already on disk.
pub fn save(server_url: &str) -> std::io::Result<()> {
    let url = server_url.to_string();
    update(move |cfg| cfg.server_url = Some(url))
}

/// Persist the registry token, preserving every other field already on disk.
/// A no-op when the stored token is already this value, so the bridge can
/// push the token on every page load without rewriting the file each time.
pub fn save_registry_token(token: &str) -> std::io::Result<()> {
    let token = token.to_string();
    update(move |cfg| cfg.registry_token = Some(token))
}

fn update(f: impl FnOnce(&mut Config)) -> std::io::Result<()> {
    let path =
        config_path().ok_or_else(|| std::io::Error::other("no config dir on this platform"))?;
    update_at(&path, f)
}

// Read-modify-write. Loading first is what keeps a `save` of one field from
// deleting the others (before LC-733 this wrote a freshly constructed Config).
fn update_at(path: &Path, f: impl FnOnce(&mut Config)) -> std::io::Result<()> {
    let mut cfg = load_at(path);
    let before = cfg.clone();
    f(&mut cfg);
    if cfg == before && path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(&cfg).map_err(std::io::Error::other)?;
    std::fs::write(path, body)?;
    // The file holds a bearer token, so keep it owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// Resolve the URL the webview should load on startup. Precedence:
//   1. LETS_CHAT_SERVER_URL env var (dev override; never written to disk).
//   2. Persisted config file.
//   3. Built-in default.
pub fn initial_server_url() -> String {
    if let Ok(url) = std::env::var("LETS_CHAT_SERVER_URL") {
        if !url.is_empty() {
            return url;
        }
    }
    if let Some(url) = load().server_url {
        if !url.is_empty() {
            return url;
        }
    }
    "http://localhost:8080".to_string()
}

/// Registry token for the update fetch, or None when the user has not signed
/// in through this app yet.
pub fn registry_token() -> Option<String> {
    load().registry_token.filter(|t| !t.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Per-test config path under the OS temp dir. The production path comes
    // from `dirs::config_dir()`, which tests must not write to, so every test
    // drives the `_at` helpers the public wrappers delegate to.
    fn temp_config_path(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "lets-chat-desktop-config-test-{}-{tag}-{n}/config.json",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// LC-733: `save` used to write a freshly constructed `Config`, so storing
    /// a server URL erased every other field. With a token in the file that
    /// meant the URL editor silently signed the updater out.
    #[test]
    fn save_preserves_fields_it_does_not_set() {
        let path = temp_config_path("preserve");
        update_at(&path, |c| c.registry_token = Some("tok-abc".into())).unwrap();
        update_at(&path, |c| {
            c.server_url = Some("https://chat.example.com".into())
        })
        .unwrap();

        let cfg = load_at(&path);
        assert_eq!(cfg.registry_token.as_deref(), Some("tok-abc"));
        assert_eq!(cfg.server_url.as_deref(), Some("https://chat.example.com"));
        cleanup(&path);
    }

    /// The reverse direction: saving a token must not drop the server URL.
    #[test]
    fn saving_a_token_preserves_the_server_url() {
        let path = temp_config_path("token");
        update_at(&path, |c| c.server_url = Some("https://a.example".into())).unwrap();
        update_at(&path, |c| c.registry_token = Some("tok-2".into())).unwrap();

        let cfg = load_at(&path);
        assert_eq!(cfg.server_url.as_deref(), Some("https://a.example"));
        assert_eq!(cfg.registry_token.as_deref(), Some("tok-2"));
        cleanup(&path);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let path = temp_config_path("missing");
        assert_eq!(load_at(&path), Config::default());
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        let path = temp_config_path("malformed");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load_at(&path), Config::default());
        cleanup(&path);
    }
}
