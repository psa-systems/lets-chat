// Persistent on-disk config for the desktop wrapper.
//
// Stores the user's chosen server URL so the GUI URL editor on the welcome
// page survives restarts without requiring the user to export
// LETS_CHAT_SERVER_URL every time. The env var stays a higher-priority dev
// override; the config file is the durable default new users land on.
//
// Format: { "server_url": "https://chat.example.com" }
//
// Location: dirs::config_dir().join("lets-chat-desktop/config.json") which
// resolves to $XDG_CONFIG_HOME/lets-chat-desktop on Linux,
// ~/Library/Application Support/lets-chat-desktop on macOS, and
// %APPDATA%\lets-chat-desktop on Windows.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub server_url: Option<String>,
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lets-chat-desktop").join("config.json"))
}

pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Config::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save(server_url: &str) -> std::io::Result<()> {
    let path =
        config_path().ok_or_else(|| std::io::Error::other("no config dir on this platform"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cfg = Config {
        server_url: Some(server_url.to_string()),
    };
    let body = serde_json::to_vec_pretty(&cfg).map_err(std::io::Error::other)?;
    std::fs::write(&path, body)
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
