#[allow(unused_imports)]
use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
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
    image_url: Option<&'a str>,
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
                image_url: cached_image.as_deref(),
            };
            return Ok(axum::response::Html(frag.render().unwrap_or_default()).into_response());
        }
    }

    // LC-152: use the shared outbound client. Its custom DNS resolver
    // filters every connection to publicly-routable IPs inside reqwest's
    // own resolution path, closing the prior check-then-reqwest-reresolves
    // TOCTOU. The manual redirect loop below (LC-150) stays — the per-hop
    // host_resolves_public pre-check is the UX gate (empty_preview()
    // fallback on rejection); the resolver is the security gate at
    // fetch-time. Both running is correct: a redirect to an internal host
    // is rejected by the resolver even if a future code change drops the
    // pre-check.
    let client = crate::http_client::outbound_no_redirects();
    // Per-request timeout overrides the helper's default.
    let request_timeout = Duration::from_secs(FETCH_TIMEOUT_SECS);

    let mut current = parsed.clone();
    let mut redirects = 0usize;
    let resp = loop {
        // Re-check on EVERY hop, not just the first: scheme (a redirect can
        // jump to file:// / gopher://) and the host's resolved IPs (reject any
        // non-globally-routable address before we connect). DNS rebinding
        // across the resolve-then-connect gap remains a small residual risk,
        // mitigated by the 5s timeout and 1 MiB body cap.
        if !matches!(current.scheme(), "http" | "https") {
            return Ok(empty_preview());
        }
        if !crate::ssrf::host_resolves_public(&current).await {
            return Ok(empty_preview());
        }
        let r = match client
            .get(current.as_str())
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
        image_url: image_url.as_deref(),
    };
    Ok(axum::response::Html(frag.render().unwrap_or_default()).into_response())
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
}
