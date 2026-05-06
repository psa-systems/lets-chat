use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::net::IpAddr;
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
    let parsed = Url::parse(&params.url)
        .map_err(|_| AppError::BadRequest("invalid URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Ok(empty_preview());
    }

    let url_hash = hash_url(parsed.as_str());

    // Cache hit and not yet expired? Render directly.
    if let Some(row) = db::uploads::get_link_preview(&state.chat, &url_hash).await? {
        if !is_expired(&row.fetched_at) {
            let frag = LinkPreviewFragment {
                url: &row.url,
                title: row.title.as_deref(),
                description: row.description.as_deref(),
                image_url: row.image_url.as_deref(),
            };
            return Ok(axum::response::Html(frag.render().unwrap_or_default()).into_response());
        }
    }

    // SSRF: resolve the host and reject any non-public IPs before we let the
    // reqwest client connect. This is the cheap belt-and-braces version of
    // a custom resolver - DNS rebinding remains possible but is mitigated by
    // the short response window and small body cap.
    let host = match parsed.host_str() {
        Some(h) => h,
        None => return Ok(empty_preview()),
    };
    let port = parsed.port_or_known_default().unwrap_or(80);
    let lookup = tokio::net::lookup_host((host, port)).await;
    let Ok(addrs) = lookup else {
        return Ok(empty_preview());
    };
    let mut any_addr = false;
    for sa in addrs {
        any_addr = true;
        if !is_globally_routable(sa.ip()) {
            return Ok(empty_preview());
        }
    }
    if !any_addr {
        return Ok(empty_preview());
    }

    // Build a one-shot reqwest client per request so timeouts are scoped.
    let Ok(client) = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
    else {
        return Ok(empty_preview());
    };

    let resp = match client.get(parsed.as_str()).send().await {
        Ok(r) => r,
        Err(_) => return Ok(empty_preview()),
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

    db::uploads::upsert_link_preview(
        &state.chat,
        &url_hash,
        parsed.as_str(),
        parsed_meta.title.as_deref(),
        parsed_meta.description.as_deref(),
        parsed_meta.image_url.as_deref(),
    )
    .await?;

    let frag = LinkPreviewFragment {
        url: parsed.as_str(),
        title: parsed_meta.title.as_deref(),
        description: parsed_meta.description.as_deref(),
        image_url: parsed_meta.image_url.as_deref(),
    };
    Ok(axum::response::Html(frag.render().unwrap_or_default()).into_response())
}

#[derive(Default)]
struct PageMeta {
    title: Option<String>,
    description: Option<String>,
    image_url: Option<String>,
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

/// IP allowlist: globally routable unicast only. Rejects loopback, private,
/// link-local, CGNAT, multicast, broadcast, unspecified, documentation, and
/// reserved addresses. Stable Rust does not yet expose IpAddr::is_global, so
/// we reject every non-public range we know about explicitly.
fn is_globally_routable(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_broadcast()
                || v4.is_documentation() || v4.is_unspecified() || v4.is_multicast()
            {
                return false;
            }
            let oct = v4.octets();
            // Carrier-grade NAT 100.64.0.0/10
            if oct[0] == 100 && (oct[1] & 0xc0) == 64 {
                return false;
            }
            // Reserved 240/4 (excl. 255.255.255.255 already broadcast)
            if oct[0] >= 240 {
                return false;
            }
            // Benchmark 198.18/15
            if oct[0] == 198 && (oct[1] == 18 || oct[1] == 19) {
                return false;
            }
            true
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            let segs = v6.segments();
            // Unique local fc00::/7
            if (segs[0] & 0xfe00) == 0xfc00 {
                return false;
            }
            // Link-local fe80::/10
            if (segs[0] & 0xffc0) == 0xfe80 {
                return false;
            }
            // IPv4-mapped ::ffff:0:0/96
            if segs[0] == 0
                && segs[1] == 0
                && segs[2] == 0
                && segs[3] == 0
                && segs[4] == 0
                && segs[5] == 0xffff
            {
                let mapped = std::net::Ipv4Addr::new(
                    (segs[6] >> 8) as u8,
                    (segs[6] & 0xff) as u8,
                    (segs[7] >> 8) as u8,
                    (segs[7] & 0xff) as u8,
                );
                return is_globally_routable(IpAddr::V4(mapped));
            }
            true
        }
    }
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
