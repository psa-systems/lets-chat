#[allow(unused_imports)]
use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
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
/// LC-985: only thumbnails up to this size are written to `image_data`; larger
/// images are served uncached.
const MAX_CACHED_IMAGE_BYTES: usize = 512 * 1024;
/// LC-985: global cap on total cached image bytes across all rows.
const MAX_CACHE_TOTAL_BYTES: i64 = 256 * 1024 * 1024;
/// LC-985: per-user, per-minute cap on each unfurl route.
const UNFURL_RATE_PER_MIN: u32 = 60;
const UNFURL_IMAGE_RATE_PER_MIN: u32 = 120;

fn check_rate(
    state: &AppState,
    kind: crate::rate_limit::RateLimitKind,
    user_id: &str,
    limit: u32,
) -> Result<(), AppError> {
    if let crate::rate_limit::Outcome::Deny { retry_after } =
        state.rate_limits.check(kind, user_id, limit)
    {
        return Err(AppError::TooManyRequests(
            "unfurl rate limit exceeded".into(),
            retry_after,
        ));
    }
    Ok(())
}

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
    AuthUser(user): AuthUser,
    Query(params): Query<UnfurlParams>,
) -> Result<Response, AppError> {
    check_rate(
        &state,
        crate::rate_limit::RateLimitKind::Unfurl,
        &user.id.to_string(),
        UNFURL_RATE_PER_MIN,
    )?;
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
///
/// LC-925: the fetched, sniffed bytes are cached on the row itself (keyed by
/// this same `url_hash`) so a second request within `PREVIEW_TTL_SECS`, from
/// any viewer, is served from the DB instead of re-fetching the remote
/// origin. `ETag: "<url_hash>"` lets a client's own revalidation short-circuit
/// to a 304 before either the cache or the remote fetch is touched.
pub async fn get_unfurl_image(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(url_hash): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    check_rate(
        &state,
        crate::rate_limit::RateLimitKind::UnfurlImage,
        &user.id.to_string(),
        UNFURL_IMAGE_RATE_PER_MIN,
    )?;
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

    let etag = format!("\"{url_hash}\"");
    if if_none_match(&headers, &etag) {
        return Ok(not_modified(&etag));
    }

    let (content_type, bytes) = match db::uploads::get_cached_image(&state.chat, &url_hash).await? {
        Some(cached) if !is_expired(&cached.fetched_at) => (cached.content_type, cached.bytes),
        _ => {
            let Some((content_type, bytes)) = fetch_image(&image_url).await else {
                return Err(AppError::NotFound);
            };
            if bytes.len() <= MAX_CACHED_IMAGE_BYTES {
                db::uploads::set_cached_image(
                    &state.chat,
                    &url_hash,
                    content_type,
                    &bytes,
                    MAX_CACHE_TOTAL_BYTES,
                )
                .await?;
            }
            (content_type.to_string(), bytes)
        }
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
            (header::ETAG, etag),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            // LC-904: belt-and-suspenders alongside the sniffed-and-allowlisted
            // Content-Type; even a browser that mis-handles the type header
            // is told this response is content to display, not to navigate to.
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"preview\"".to_string(),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// True when one of the client's `If-None-Match` tags equals `etag`, or the
/// wildcard `*`. Mirrors `routes::avatar::if_none_match`.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').any(|t| {
                let t = t.trim();
                t == etag || t == "*"
            })
        })
}

fn not_modified(etag: &str) -> Response {
    (
        StatusCode::NOT_MODIFIED,
        [
            (
                header::CACHE_CONTROL,
                format!("private, max-age={PREVIEW_TTL_SECS}"),
            ),
            (header::ETAG, etag.to_string()),
        ],
    )
        .into_response()
}

/// LC-857: fetch a remote image through the SSRF-guarded client, following
/// redirects, and capping the body. LC-904: the served Content-Type is
/// determined by sniffing the downloaded bytes against
/// `crate::uploads::ALLOWED_IMAGE_MIME`, never by trusting the remote
/// server's claimed header (a remote can claim `image/png` for an SVG
/// document with an inline `<script>`, and this route serves same-origin).
/// Returns `(sniffed_content_type, bytes)` or `None` on any failure/rejection.
/// Mirrors the unfurl fetch loop (`http_client::outbound_get` re-checks SSRF
/// per hop).
async fn fetch_image(url: &str) -> Option<(&'static str, Vec<u8>)> {
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
    // Cheap advisory pre-filter only: a foreign server's claimed Content-Type
    // is not trusted for anything past "is this worth downloading at all".
    // Drop any `; charset=` parameter before comparing.
    let claimed_ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !claimed_ctype.starts_with("image/") {
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
    let content_type = sniff_served_content_type(&body)?;
    Some((content_type, body))
}

/// LC-904: sniff `body`'s magic bytes and return the served Content-Type only
/// if it is in the shared raster allowlist, independent of whatever the
/// remote server's Content-Type header claimed. Returns `None` when the sniff
/// yields nothing (unrecognized bytes) or a type outside the allowlist -
/// notably `image/svg+xml`, which `infer` (and browsers) treat as an XML
/// document rather than a raster image.
fn sniff_served_content_type(body: &[u8]) -> Option<&'static str> {
    let sniffed = infer::get(body)?.mime_type();
    crate::uploads::ALLOWED_IMAGE_MIME
        .iter()
        .find(|&&allowed| allowed == sniffed)
        .copied()
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

    fn tiny_png() -> Vec<u8> {
        use image::ImageEncoder;
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let mut buf = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(&img, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        buf
    }

    const SVG_WITH_SCRIPT: &[u8] =
        br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(document.cookie)</script></svg>"#;

    #[test]
    fn svg_body_is_refused_regardless_of_claimed_header() {
        // LC-904: the served type is a function of the BYTES, never of the
        // remote server's claimed Content-Type. An SVG document (which a
        // browser renders and scripts, not just displays) must be refused
        // whether the foreign server claimed `image/svg+xml` or lied and
        // claimed `image/png`; `sniff_served_content_type` takes no header
        // argument at all, so both claims collapse to the same sniff.
        assert_eq!(sniff_served_content_type(SVG_WITH_SCRIPT), None);
    }

    #[test]
    fn png_body_is_served_as_png_even_if_header_claims_otherwise() {
        // The served type comes from sniffing, so a remote claiming
        // `image/jpeg` for actual PNG bytes still serves as `image/png`.
        assert_eq!(sniff_served_content_type(&tiny_png()), Some("image/png"));
    }

    #[test]
    fn unrecognized_bytes_are_refused() {
        assert_eq!(sniff_served_content_type(b"not an image at all"), None);
    }

    #[test]
    fn conditional_get_matches_etag_or_wildcard() {
        let etag = "\"deadbeef\"";
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        assert!(if_none_match(&headers, etag));

        let mut wildcard = axum::http::HeaderMap::new();
        wildcard.insert(header::IF_NONE_MATCH, "*".parse().unwrap());
        assert!(if_none_match(&wildcard, etag));

        let mut mismatched = axum::http::HeaderMap::new();
        mismatched.insert(header::IF_NONE_MATCH, "\"other\"".parse().unwrap());
        assert!(!if_none_match(&mismatched, etag));

        assert!(!if_none_match(&axum::http::HeaderMap::new(), etag));
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
