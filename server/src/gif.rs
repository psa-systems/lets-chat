//! LC-488: Tenor GIF picker.
//!
//! Config is env-only (an API key), read on demand - there is no client object
//! in `AppState`. The picker searches Tenor; on pick, the server fetches the
//! chosen GIF and stores it through the uploads pipeline so it serves
//! same-origin (a posted GIF is a normal attachment, never hotlinked). The
//! fetch is confined to the Tenor CDN by [`is_tenor_media_url`] on top of the
//! `http_client` public-IP SSRF filter.

/// Operator Tenor configuration. `from_env` returns `None` when
/// `LETS_CHAT_TENOR_API_KEY` is unset/empty, which hides the GIF picker.
#[derive(Debug, Clone)]
pub struct GifConfig {
    pub api_key: String,
    /// Tenor's recommended per-integration key; defaults to "lets-chat".
    pub client_key: String,
    /// Tenor content filter: off / low / medium / high. Defaults to "medium".
    pub content_filter: String,
}

impl GifConfig {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("LETS_CHAT_TENOR_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let client_key = std::env::var("LETS_CHAT_TENOR_CLIENT_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "lets-chat".to_string());
        let content_filter = std::env::var("LETS_CHAT_TENOR_CONTENT_FILTER")
            .ok()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| matches!(s.as_str(), "off" | "low" | "medium" | "high"))
            .unwrap_or_else(|| "medium".to_string());
        Some(Self {
            api_key,
            client_key,
            content_filter,
        })
    }
}

/// Whether the GIF picker is enabled (`LETS_CHAT_TENOR_API_KEY` is set).
pub fn available() -> bool {
    GifConfig::from_env().is_some()
}

/// One search result. `preview_url` is a small GIF shown in the picker grid
/// (hotlinked to Tenor's CDN); `gif_url` is the full GIF the server fetches +
/// stores when the user picks it.
#[derive(Debug, Clone)]
pub struct GifResult {
    pub id: String,
    pub preview_url: String,
    pub gif_url: String,
    pub description: String,
}

/// Parse a Tenor v2 `/search` (or `/featured`) response into results. Pure, so
/// it is unit-tested without a live Tenor call.
pub fn parse_search(body: &serde_json::Value) -> Vec<GifResult> {
    let mut out = Vec::new();
    let Some(results) = body.get("results").and_then(|r| r.as_array()) else {
        return out;
    };
    for r in results {
        let id = r
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let formats = r.get("media_formats");
        let preview_url = pick_format(formats, &["tinygif", "nanogif", "gif"]);
        let gif_url = pick_format(formats, &["gif", "mediumgif", "tinygif"]);
        let description = r
            .get("content_description")
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

/// First non-empty `media_formats[<pref>].url` for the preference order.
fn pick_format(formats: Option<&serde_json::Value>, prefs: &[&str]) -> String {
    let Some(f) = formats else {
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

/// Allowlist for a GIF URL the server will fetch + store: https on the Tenor
/// CDN only. With the `http_client` SSRF public-IP filter this keeps the fetch
/// confined to the provider, so a client cannot steer it at an arbitrary host.
pub fn is_tenor_media_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => {
            u.scheme() == "https"
                && matches!(u.host_str(), Some(h) if h == "tenor.com" || h.ends_with(".tenor.com"))
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
            "results": [
                {
                    "id": "123",
                    "content_description": "happy cat",
                    "media_formats": {
                        "tinygif": { "url": "https://media.tenor.com/tiny.gif" },
                        "gif": { "url": "https://media.tenor.com/full.gif" }
                    }
                }
            ]
        });
        let got = parse_search(&body);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "123");
        assert_eq!(got[0].preview_url, "https://media.tenor.com/tiny.gif");
        assert_eq!(got[0].gif_url, "https://media.tenor.com/full.gif");
        assert_eq!(got[0].description, "happy cat");
    }

    #[test]
    fn parse_falls_back_and_skips_incomplete() {
        let body = serde_json::json!({
            "results": [
                // only nanogif -> used for preview AND (via fallback) nothing for gif -> skipped
                { "id": "a", "media_formats": { "nanogif": { "url": "https://media.tenor.com/n.gif" } } },
                // no media_formats -> skipped
                { "id": "b" },
                // complete via mediumgif fallback for the full url
                { "id": "c", "media_formats": {
                    "tinygif": { "url": "https://media.tenor.com/c-tiny.gif" },
                    "mediumgif": { "url": "https://media.tenor.com/c-med.gif" }
                } }
            ]
        });
        let got = parse_search(&body);
        // "a" has only nanogif (no full-gif url) -> skipped; "b" has no formats
        // -> skipped; "c" is complete via the mediumgif fallback for the full url.
        let ids: Vec<&str> = got.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["c"]);
        assert_eq!(got[0].gif_url, "https://media.tenor.com/c-med.gif");
        assert_eq!(got[0].preview_url, "https://media.tenor.com/c-tiny.gif");
    }

    #[test]
    fn parse_empty_or_malformed_is_empty() {
        assert!(parse_search(&serde_json::json!({})).is_empty());
        assert!(parse_search(&serde_json::json!({ "results": "nope" })).is_empty());
    }

    #[test]
    fn tenor_url_allowlist() {
        assert!(is_tenor_media_url("https://media.tenor.com/x.gif"));
        assert!(is_tenor_media_url("https://media1.tenor.com/x.gif"));
        assert!(is_tenor_media_url("https://tenor.com/x.gif"));
        // rejected: wrong scheme, look-alike host, non-tenor, garbage
        assert!(!is_tenor_media_url("http://media.tenor.com/x.gif"));
        assert!(!is_tenor_media_url("https://eviltenor.com/x.gif"));
        assert!(!is_tenor_media_url("https://tenor.com.evil.com/x.gif"));
        assert!(!is_tenor_media_url("https://example.com/x.gif"));
        assert!(!is_tenor_media_url("not a url"));
    }
}
