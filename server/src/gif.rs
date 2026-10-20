//! LC-488 / LC-505: GIF picker (Giphy).
//!
//! Config is env-only (an API key), read on demand - there is no client object
//! in `AppState`. The picker searches Giphy; on pick, the server fetches the
//! chosen GIF and stores it through the uploads pipeline so it serves
//! same-origin (a posted GIF is a normal attachment, never hotlinked). The
//! fetch is confined to the Giphy CDN by [`is_giphy_media_url`] on top of the
//! `http_client` public-IP SSRF filter.
//!
//! Originally targeted Tenor; Google shut down the public Tenor API
//! (announced 2026-01-13, full shutdown 2026-06-30), so LC-505 migrated the
//! adapter to Giphy. Only this provider adapter is Giphy-specific.

/// Operator Giphy configuration. `from_env` returns `None` when
/// `LETS_CHAT_GIPHY_API_KEY` is unset/empty, which hides the GIF picker.
#[derive(Debug, Clone)]
pub struct GifConfig {
    pub api_key: String,
    /// Giphy content rating: g / pg / pg-13 / r. Defaults to "pg-13".
    pub rating: String,
}

/// LC-1065: settings-store keys for the admin-managed GIF config. The API key
/// is stored AES-256-GCM-sealed (hex `nonce:ciphertext`) under the process
/// secret key, exactly like the IMAP password; the rating is a plain validated
/// string.
pub const GIPHY_KEY_SETTING: &str = "giphy_api_key";
pub const GIPHY_RATING_SETTING: &str = "giphy_rating";

/// Validated Giphy content rating, lowercased, or `None` if not one of the
/// four Giphy values.
fn normalize_rating(raw: &str) -> Option<String> {
    let r = raw.trim().to_lowercase();
    matches!(r.as_str(), "g" | "pg" | "pg-13" | "r").then_some(r)
}

/// LC-1065: seal `api_key` into the `hex(nonce):hex(ciphertext)` form stored in
/// the settings table. The admin settings handler calls this.
pub fn seal_api_key(
    secret_key: &[u8; 32],
    api_key: &str,
) -> Result<String, crate::crypto::CryptoError> {
    let (ciphertext, nonce) = crate::crypto::seal(secret_key, api_key.as_bytes())?;
    Ok(format!(
        "{}:{}",
        hex::encode(nonce),
        hex::encode(ciphertext)
    ))
}

/// Reverse of [`seal_api_key`]; `None` on any malformed or undecryptable value
/// (e.g. the secret key was rotated after the key was sealed).
fn unseal_api_key(secret_key: &[u8; 32], stored: &str) -> Option<String> {
    let (nonce_hex, ct_hex) = stored.split_once(':')?;
    let nonce = hex::decode(nonce_hex).ok()?;
    let ciphertext = hex::decode(ct_hex).ok()?;
    let bytes = crate::crypto::open(secret_key, &nonce, &ciphertext).ok()?;
    String::from_utf8(bytes).ok()
}

impl GifConfig {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("LETS_CHAT_GIPHY_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let rating = std::env::var("LETS_CHAT_GIPHY_RATING")
            .ok()
            .and_then(|s| normalize_rating(&s))
            .unwrap_or_else(|| "pg-13".to_string());
        Some(Self { api_key, rating })
    }

    /// LC-1065: the active GIF config. The admin-managed DB setting wins; the
    /// `LETS_CHAT_GIPHY_API_KEY` env var is the fallback, so existing env
    /// deployments keep working. Read live per request (no AppState cache), so
    /// an admin change takes effect without a restart.
    pub async fn resolve(state: &crate::state::AppState) -> Option<Self> {
        if let Some(cfg) = Self::from_settings(state).await {
            return Some(cfg);
        }
        Self::from_env()
    }

    async fn from_settings(state: &crate::state::AppState) -> Option<Self> {
        let secret_key = state.secret_key.as_ref()?;
        let stored = crate::db::settings::get_setting(&state.settings, GIPHY_KEY_SETTING)
            .await
            .ok()
            .flatten()?;
        let api_key = unseal_api_key(secret_key, &stored)?.trim().to_string();
        if api_key.is_empty() {
            return None;
        }
        let rating = crate::db::settings::get_setting(&state.settings, GIPHY_RATING_SETTING)
            .await
            .ok()
            .flatten()
            .and_then(|s| normalize_rating(&s))
            .unwrap_or_else(|| "pg-13".to_string());
        Some(Self { api_key, rating })
    }

    /// LC-1065: whether an admin has stored a usable sealed key in the settings
    /// table (independent of the env fallback), so the admin page can show the
    /// current source without rendering the plaintext.
    pub async fn settings_key_present(state: &crate::state::AppState) -> bool {
        let Some(secret_key) = state.secret_key.as_ref() else {
            return false;
        };
        match crate::db::settings::get_setting(&state.settings, GIPHY_KEY_SETTING).await {
            Ok(Some(stored)) => unseal_api_key(secret_key, &stored)
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false),
            _ => false,
        }
    }
}

/// Whether the GIF picker is enabled (an admin-stored key, else the env key).
pub async fn available(state: &crate::state::AppState) -> bool {
    GifConfig::resolve(state).await.is_some()
}

/// One search result. `preview_url` is a small GIF shown in the picker grid
/// (hotlinked to Giphy's CDN); `gif_url` is the GIF the server fetches +
/// stores when the user picks it.
#[derive(Debug, Clone)]
pub struct GifResult {
    pub id: String,
    pub preview_url: String,
    pub gif_url: String,
    pub description: String,
}

/// Parse a Giphy v1 `/gifs/search` (or `/trending`) response into results.
/// Pure, so it is unit-tested without a live Giphy call.
pub fn parse_search(body: &serde_json::Value) -> Vec<GifResult> {
    let mut out = Vec::new();
    let Some(data) = body.get("data").and_then(|r| r.as_array()) else {
        return out;
    };
    for r in data {
        let id = r
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let images = r.get("images");
        // Small render in the grid; downsized for the posted GIF keeps the
        // fetched bytes modest (Giphy's "original" can be many MB).
        let preview_url = pick_format(
            images,
            &[
                "fixed_height_small",
                "fixed_width_small",
                "preview_gif",
                "downsized",
            ],
        );
        let gif_url = pick_format(images, &["downsized", "original", "fixed_height"]);
        let description = r
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("GIF")
            .to_string();
        if id.is_empty() || preview_url.is_empty() || gif_url.is_empty() {
            continue;
        }
        out.push(GifResult {
            id,
            preview_url,
            gif_url,
            description,
        });
    }
    out
}

/// First non-empty `images[<pref>].url` for the preference order.
fn pick_format(images: Option<&serde_json::Value>, prefs: &[&str]) -> String {
    let Some(f) = images else {
        return String::new();
    };
    for k in prefs {
        if let Some(u) = f.get(k).and_then(|v| v.get("url")).and_then(|v| v.as_str()) {
            if !u.is_empty() {
                return u.to_string();
            }
        }
    }
    String::new()
}

/// Allowlist for a GIF URL the server will fetch + store: https on the Giphy
/// CDN only. With the `http_client` SSRF public-IP filter this keeps the fetch
/// confined to the provider, so a client cannot steer it at an arbitrary host.
pub fn is_giphy_media_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => {
            u.scheme() == "https"
                && matches!(u.host_str(), Some(h) if h == "giphy.com" || h.ends_with(".giphy.com"))
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_round_trips_the_api_key() {
        // LC-1065: the sealed form stored in settings decrypts back to the key.
        let key = [7u8; 32];
        let sealed = seal_api_key(&key, "gk_live_abc123").unwrap();
        // hex "nonce:ciphertext", no plaintext leaked.
        assert!(sealed.contains(':'));
        assert!(!sealed.contains("gk_live_abc123"));
        assert_eq!(
            unseal_api_key(&key, &sealed).as_deref(),
            Some("gk_live_abc123")
        );
    }

    #[test]
    fn unseal_rejects_garbage_and_the_wrong_key() {
        let key = [7u8; 32];
        let sealed = seal_api_key(&key, "secret").unwrap();
        assert_eq!(unseal_api_key(&key, "not-sealed"), None);
        assert_eq!(unseal_api_key(&key, "zz:zz"), None);
        // A rotated/different secret key cannot open it.
        assert_eq!(unseal_api_key(&[9u8; 32], &sealed), None);
    }

    #[test]
    fn rating_validation_accepts_only_the_four_giphy_values() {
        assert_eq!(normalize_rating("PG-13").as_deref(), Some("pg-13"));
        assert_eq!(normalize_rating(" g ").as_deref(), Some("g"));
        assert_eq!(normalize_rating("x"), None);
        assert_eq!(normalize_rating(""), None);
    }

    #[test]
    fn parse_picks_preview_and_full_urls() {
        let body = serde_json::json!({
            "data": [
                {
                    "id": "123",
                    "title": "happy cat",
                    "images": {
                        "fixed_height_small": { "url": "https://media.giphy.com/tiny.gif" },
                        "downsized": { "url": "https://media.giphy.com/down.gif" },
                        "original": { "url": "https://media.giphy.com/orig.gif" }
                    }
                }
            ]
        });
        let got = parse_search(&body);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "123");
        assert_eq!(got[0].preview_url, "https://media.giphy.com/tiny.gif");
        // downsized preferred over original for the posted GIF.
        assert_eq!(got[0].gif_url, "https://media.giphy.com/down.gif");
        assert_eq!(got[0].description, "happy cat");
    }

    #[test]
    fn parse_falls_back_and_skips_incomplete() {
        let body = serde_json::json!({
            "data": [
                // only preview_gif -> preview ok but no full-gif format -> skipped
                { "id": "a", "images": { "preview_gif": { "url": "https://media.giphy.com/a.gif" } } },
                // no images -> skipped
                { "id": "b" },
                // complete: original used for the full url (downsized absent)
                { "id": "c", "images": {
                    "fixed_width_small": { "url": "https://media.giphy.com/c-tiny.gif" },
                    "original": { "url": "https://media.giphy.com/c-orig.gif" }
                } }
            ]
        });
        let got = parse_search(&body);
        let ids: Vec<&str> = got.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["c"]);
        assert_eq!(got[0].gif_url, "https://media.giphy.com/c-orig.gif");
        assert_eq!(got[0].preview_url, "https://media.giphy.com/c-tiny.gif");
    }

    #[test]
    fn parse_empty_or_malformed_is_empty() {
        assert!(parse_search(&serde_json::json!({})).is_empty());
        assert!(parse_search(&serde_json::json!({ "data": "nope" })).is_empty());
    }

    #[test]
    fn giphy_url_allowlist() {
        assert!(is_giphy_media_url("https://media.giphy.com/x.gif"));
        assert!(is_giphy_media_url("https://media0.giphy.com/x.gif"));
        assert!(is_giphy_media_url("https://i.giphy.com/x.gif"));
        assert!(is_giphy_media_url("https://giphy.com/x.gif"));
        // rejected: wrong scheme, look-alike host, non-giphy, garbage
        assert!(!is_giphy_media_url("http://media.giphy.com/x.gif"));
        assert!(!is_giphy_media_url("https://evilgiphy.com/x.gif"));
        assert!(!is_giphy_media_url("https://giphy.com.evil.com/x.gif"));
        assert!(!is_giphy_media_url("https://example.com/x.gif"));
        assert!(!is_giphy_media_url("not a url"));
    }
}
