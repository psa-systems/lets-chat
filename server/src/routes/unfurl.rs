#[allow(unused_imports)]
use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use url::Url;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

const PREVIEW_TTL_SECS: i64 = 24 * 60 * 60;
const MAX_PREVIEW_BYTES: usize = 1024 * 1024;
/// LC-857: a preview thumbnail is small; 5 MiB is generous headroom while
/// bounding what one proxied fetch can pull into memory.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 5;
const USER_AGENT: &str = "lets-chat-unfurler/1.0";
const MAX_REDIRECTS: usize = 3;

#[derive(Deserialize)]
pub struct UnfurlParams {
    pub url: String,
}

#[derive(Template)]
#[template(path = "partials/link_preview.html")]
struct LinkPreviewFragment<'a> {
    url: &'a str,
    title: Option<&'a str>,
    description: Option<&'a str>,
    /// LC-857: the preview's `url_hash`, present only when a usable image
    /// exists. The template renders `/api/unfurl/image/{hash}` from it so the
    /// thumbnail is served same-origin (CSP `img-src 'self'`) instead of
    /// hotlinking the remote `og:image`, which the CSP blocks. The raw image
    /// URL stays in the DB row; the proxy reads it there by this hash.
    image_hash: Option<&'a str>,
}

/// `GET /api/unfurl?url=...` - server-side fetch of an external URL,
/// returning a rendered HTML preview card that HTMX swaps inline. AuthUser
/// gated to prevent anonymous abuse. Hardened against SSRF: only http/https
/// schemes, only globally routable IPs, 5s timeout, 1 MiB body cap, only
/// text/html parsed. Cached 24h in `link_previews` keyed by URL hash.
pub async fn get_unfurl(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(params): Query<UnfurlParams>,
) -> Result<Response, AppError> {
    let parsed = Url::parse(&params.url).map_err(|_| AppError::BadRequest("invalid URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Ok(empty_preview());
    }

    let url_hash = hash_url(parsed.as_str());

    // Cache hit and not yet expired? Render directly.
    if let Some(row) = db::uploads::get_link_preview(&state.chat, &url_hash).await? {
        if !is_expired(&row.fetched_at) {
            // LC-155: rows written before the og:image scheme guard (or by any
            // future writer) are re-sanitized on read, so a stale row cannot
            // surface a non-http(s) image source. Resolve against the row's URL.
            let cached_image = row.image_url.as_deref().and_then(|raw| {
                Url::parse(&row.url)
                    .ok()
                    .and_then(|base| sanitize_image_url(&base, raw))
            });
            let frag = LinkPreviewFragment {
                url: &row.url,
                title: row.title.as_deref(),
                description: row.description.as_deref(),
                // The thumbnail is served through the proxy keyed by this row's
                // hash, but only when the (re-sanitized) image survived.
                image_hash: cached_image.as_ref().map(|_| url_hash.as_str()),
            };
            return Ok(axum::response::Html(frag.render().unwrap_or_default()).into_response());
        }
    }

    // LC-152: every hop's URL goes through `http_client::outbound_get`, which
    // applies the two-layer SSRF guard:
    //
    //   - **URL-input validation**: parses, rejects non-`http(s)` schemes,
    //     and calls `ssrf::host_resolves_public` to filter literal IPs and
    //     hostname-resolves-private. This catches the literal-IP bypass
    //     (e.g. a 302 to `http://127.0.0.1/`) that reqwest's custom resolver
    //     would NOT see (literal-IP URLs skip `dns::Resolve` entirely).
    //   - **`PublicOnlyResolver`** inside reqwest's resolution path catches
    //     the hostname use-time TOCTOU. Each redirect connection re-invokes
    //     the resolver, so the per-hop guarantee applies per connection.
    //
    // The manual redirect loop (LC-150) stays because the unfurl path
    // needs the empty_preview() control-flow fallback on rejection; using
    // reqwest's redirect-following would lose that.
    let request_timeout = Duration::from_secs(FETCH_TIMEOUT_SECS);

    let mut current = parsed.clone();
    let mut redirects = 0usize;
    let resp = loop {
        let req = match crate::http_client::outbound_get(current.as_str()).await {
            Ok(r) => r,
            Err(_) => return Ok(empty_preview()),
        };
        let r = match req
            .timeout(request_timeout)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Ok(empty_preview()),
        };
        if r.status().is_redirection() {
            let Some(loc) = r
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|l| !l.is_empty())
            else {
                // No / empty Location: a 3xx with nothing to follow. Bail
                // rather than `join("")` back onto the same URL (which would
                // just burn redirect budget re-fetching it).
                return Ok(empty_preview());
            };
            // Resolve relative redirects against the current URL.
            let Ok(next) = current.join(loc) else {
                return Ok(empty_preview());
            };
            redirects += 1;
            if redirects > MAX_REDIRECTS {
                return Ok(empty_preview());
            }
            current = next;
            continue;
        }
        break r;
    };
    if !resp.status().is_success() {
        return Ok(empty_preview());
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if !ctype.contains("text/html") {
        return Ok(empty_preview());
    }

    // Stream the body with a hard cap so we don't blow memory on huge pages.
    let mut body = Vec::with_capacity(64 * 1024);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(bytes) = chunk else { break };
        if body.len() + bytes.len() > MAX_PREVIEW_BYTES {
            // Capture as much as we can and stop; OG tags live in <head> so
            // we likely have enough already.
            let take = MAX_PREVIEW_BYTES - body.len();
            body.extend_from_slice(&bytes[..take]);
            break;
        }
        body.extend_from_slice(&bytes);
    }

    let html_str = match std::str::from_utf8(&body) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(&body).into_owned(),
    };
    let parsed_meta = parse_meta(&html_str);

    // LC-155: the `og:image` comes from the remote page and lands in an
    // `<img src>`. Resolve it against the final page URL and keep it only if
    // it is an absolute http/https URL, so a page cannot inject a
    // `javascript:` / `data:` (or other-scheme) image source. Dropped rather
    // than neutralized: a preview with no image is fine.
    let image_url = parsed_meta
        .image_url
        .as_deref()
        .and_then(|raw| sanitize_image_url(&current, raw));

    db::uploads::upsert_link_preview(
        &state.chat,
        &url_hash,
        parsed.as_str(),
        parsed_meta.title.as_deref(),
        parsed_meta.description.as_deref(),
        image_url.as_deref(),
    )
    .await?;

    let frag = LinkPreviewFragment {
        url: parsed.as_str(),
        title: parsed_meta.title.as_deref(),
        description: parsed_meta.description.as_deref(),
        image_hash: image_url.as_ref().map(|_| url_hash.as_str()),
    };
    Ok(axum::response::Html(frag.render().unwrap_or_default()).into_response())
}

/// `GET /api/unfurl/image/{url_hash}` - LC-857: serve a link preview's thumbnail
/// same-origin so it passes the CSP `img-src 'self'` (the remote `og:image` is
/// blocked when hotlinked). AuthUser-gated like the unfurl endpoint. Not an open
/// proxy: it fetches ONLY the `image_url` already stored on the `link_previews`
/// row for this hash (a URL the unfurler already fetched and sanitized), not an
/// arbitrary caller-supplied URL. Any failure is a 404, never a 5xx, so a dead
/// thumbnail reads as "no image" (and the template's onerror hides the box).
pub async fn get_unfurl_image(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(url_hash): Path<String>,
) -> Result<Response, AppError> {
    if !is_valid_hash(&url_hash) {
        return Err(AppError::NotFound);
    }
    let Some(row) = db::uploads::get_link_preview(&state.chat, &url_hash).await? else {
        return Err(AppError::NotFound);
    };
    // Re-sanitize the stored URL against the row's own URL, exactly as the
    // render path does: a row written before the scheme guard cannot surface a
    // non-http(s) source, and what we fetch is exactly what the card points at.
    let Some(image_url) = row.image_url.as_deref().and_then(|raw| {
        Url::parse(&row.url)
            .ok()
            .and_then(|base| sanitize_image_url(&base, raw))
    }) else {
        return Err(AppError::NotFound);
    };
    let Some((content_type, bytes)) = fetch_image(&image_url).await else {
        return Err(AppError::NotFound);
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            // Match the preview row's 24h TTL. Private: it is per-viewer
            // AuthUser-gated content, so it must not sit in a shared cache.
            (
                header::CACHE_CONTROL,
                format!("private, max-age={PREVIEW_TTL_SECS}"),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// LC-857: fetch a remote image through the SSRF-guarded client, following
/// redirects, gating on an `image/*` content-type and capping the body. Returns
/// `(content_type, bytes)` or `None` on any failure/rejection. Mirrors the
/// unfurl fetch loop (`http_client::outbound_get` re-checks SSRF per hop).
async fn fetch_image(url: &str) -> Option<(String, Vec<u8>)> {
    let start = Url::parse(url).ok()?;
    if !matches!(start.scheme(), "http" | "https") {
        return None;
    }
    let timeout = Duration::from_secs(FETCH_TIMEOUT_SECS);
    let mut current = start;
    let mut redirects = 0usize;
    let resp = loop {
        let req = crate::http_client::outbound_get(current.as_str())
            .await
            .ok()?;
        let r = req
            .timeout(timeout)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .ok()?;
        if r.status().is_redirection() {
            let loc = r
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|l| !l.is_empty())?;
            let next = current.join(loc).ok()?;
            redirects += 1;
            if redirects > MAX_REDIRECTS {
                return None;
            }
            current = next;
            continue;
        }
        break r;
    };
    if !resp.status().is_success() {
        return None;
    }
    // Only real images; drop any `; charset=` parameter for the served value.
    let base_ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !base_ctype.starts_with("image/") {
        return None;
    }
    let mut body = Vec::with_capacity(64 * 1024);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.ok()?;
        // Refuse an oversized image rather than truncate (a partial image is a
        // broken image, not a smaller one).
        if body.len() + bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
        body.extend_from_slice(&bytes);
    }
    if body.is_empty() {
        return None;
    }
    Some((base_ctype, body))
}

/// LC-857: a `url_hash` is a lowercase sha256 hex string (see `hash_url`).
/// Reject anything else so a malformed path can never reach the DB lookup.
fn is_valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Default)]
struct PageMeta {
    title: Option<String>,
    description: Option<String>,
    image_url: Option<String>,
}

/// LC-155: resolve an `og:image` against the page URL and accept it only if it
/// is an absolute http/https URL. Relative paths resolve against `base`;
/// absolute non-http(s) schemes (`javascript:`, `data:`, ...) are rejected so
/// they cannot reach an `<img src>`.
fn sanitize_image_url(base: &Url, raw: &str) -> Option<String> {
    let resolved = base.join(raw.trim()).ok()?;
    matches!(resolved.scheme(), "http" | "https").then(|| resolved.to_string())
}

fn parse_meta(html_str: &str) -> PageMeta {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html_str);
    let mut meta = PageMeta::default();

    let pick = |doc: &Html, sel: &str, attr: &str| -> Option<String> {
        let s = Selector::parse(sel).ok()?;
        for el in doc.select(&s) {
            if let Some(v) = el.value().attr(attr) {
                let trimmed = v.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    };

    meta.title = pick(&doc, r#"meta[property="og:title"]"#, "content")
        .or_else(|| pick(&doc, r#"meta[name="twitter:title"]"#, "content"))
        .or_else(|| {
            Selector::parse("title").ok().and_then(|s| {
                doc.select(&s)
                    .next()
                    .map(|el| el.text().collect::<String>().trim().to_string())
                    .filter(|t| !t.is_empty())
            })
        });
    meta.description = pick(&doc, r#"meta[property="og:description"]"#, "content")
        .or_else(|| pick(&doc, r#"meta[name="twitter:description"]"#, "content"))
        .or_else(|| pick(&doc, r#"meta[name="description"]"#, "content"));
    meta.image_url = pick(&doc, r#"meta[property="og:image"]"#, "content")
        .or_else(|| pick(&doc, r#"meta[name="twitter:image"]"#, "content"));
    meta
}

fn hash_url(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn is_expired(fetched_at: &str) -> bool {
    let parsed = chrono::NaiveDateTime::parse_from_str(fetched_at, "%Y-%m-%d %H:%M:%S")
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    let Ok(ts) = parsed else { return true };
    let age = (Utc::now() - ts).num_seconds();
    age > PREVIEW_TTL_SECS
}

fn empty_preview() -> Response {
    (StatusCode::OK, axum::response::Html(String::new())).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/page").unwrap()
    }

    #[test]
    fn image_url_accepts_absolute_and_relative_http() {
        assert_eq!(
            sanitize_image_url(&base(), "https://cdn.example.com/a.png").as_deref(),
            Some("https://cdn.example.com/a.png")
        );
        // Relative resolves against the page URL.
        assert_eq!(
            sanitize_image_url(&base(), "/img/b.png").as_deref(),
            Some("https://example.com/img/b.png")
        );
    }

    #[test]
    fn image_url_rejects_dangerous_schemes() {
        for raw in [
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "vbscript:x",
            "file:///etc/passwd",
        ] {
            assert!(
                sanitize_image_url(&base(), raw).is_none(),
                "{raw} must be rejected"
            );
        }
    }

    #[test]
    fn image_proxy_hash_validation_is_strict() {
        // A real sha256 hex (what hash_url emits) is accepted.
        assert!(is_valid_hash(&hash_url("https://example.com/x")));
        assert!(is_valid_hash(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        // Wrong length, uppercase, non-hex, and path-traversal shapes are not.
        assert!(!is_valid_hash(""));
        assert!(!is_valid_hash(&"a".repeat(63)));
        assert!(!is_valid_hash(&"a".repeat(65)));
        assert!(!is_valid_hash(
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        ));
        assert!(!is_valid_hash(
            "../../etc/passwd-padded-out-to-sixty-four-bytes-for-this-test!!!"
        ));
    }
}
