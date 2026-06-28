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

impl GifConfig {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("LETS_CHAT_GIPHY_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let rating = std::env::var("LETS_CHAT_GIPHY_RATING")
            .ok()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| matches!(s.as_str(), "g" | "pg" | "pg-13" | "r"))
            .unwrap_or_else(|| "pg-13".to_string());
        Some(Self { api_key, rating })
    }
}

/// Whether the GIF picker is enabled (`LETS_CHAT_GIPHY_API_KEY` is set).
pub fn available() -> bool {
    GifConfig::from_env().is_some()
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
