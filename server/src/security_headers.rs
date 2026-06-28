//! LC-504: security response headers.
//!
//! lets-chat historically returned no transport/framing/sniffing/policy
//! headers on any response (only the Secure/SameSite session cookie was
//! hardened). [`set_security_headers`] is a tower middleware applied as the
//! OUTERMOST layer of [`crate::routes::build_router`], so EVERY response -
//! HTML pages, HTMX fragments, the JSON API, webhooks, feeds, static assets,
//! redirects and error pages alike - carries the six headers below.
//!
//! The header set is fixed (no per-request work), so each value is a
//! `'static` [`HeaderValue`] built once. We only `insert` a header when the
//! handler did not already set one, so a route that needs a looser policy
//! (none today) can still override.

use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// `Strict-Transport-Security`. One year, subdomains, preload-eligible. Only
/// honored by browsers over HTTPS; harmless (ignored) over plaintext, so it is
/// safe to set unconditionally.
const HSTS: HeaderValue = HeaderValue::from_static("max-age=31536000; includeSubDomains; preload");

/// `X-Content-Type-Options`. Stops MIME sniffing of responses.
const NOSNIFF: HeaderValue = HeaderValue::from_static("nosniff");

/// `X-Frame-Options`. Belt-and-suspenders with CSP `frame-ancestors 'none'`
/// for older browsers; the app is never meant to be framed (the desktop
/// Tao+Wry webview loads it top-level, not in an iframe).
const FRAME_OPTIONS: HeaderValue = HeaderValue::from_static("DENY");

/// `Referrer-Policy`. Send the full URL same-origin, only the origin
/// cross-origin, and nothing when downgrading to HTTP.
const REFERRER_POLICY: HeaderValue = HeaderValue::from_static("strict-origin-when-cross-origin");

/// `Permissions-Policy`. Deny powerful features by default; allow
/// camera/microphone/screen-capture from same-origin so the WebRTC call
/// surface (getUserMedia + screen share) keeps working. Geolocation and
/// other features the app never uses are denied outright.
const PERMISSIONS_POLICY: HeaderValue = HeaderValue::from_static(
    "camera=(self), microphone=(self), display-capture=(self), \
     geolocation=(), payment=(), usb=(), interest-cohort=()",
);

/// `Content-Security-Policy`. Tuned to the existing HTMX/Askama app, NOT a
/// from-scratch lockdown, so it cannot break shipped functionality:
/// - `'unsafe-inline'` (script + style) because templates carry inline
///   `<script>` blocks, `hx-on::*` attributes and inline `style=`/branding
///   CSS-var blocks.
/// - `'unsafe-eval'` because htmx evaluates `hx-on::*` handlers via the
///   `Function` constructor.
/// - `img-src` allows `data:`/`blob:` (avatar/upload previews) and the Tenor
///   CDN (`*.tenor.com`) because the LC-488 GIF picker grid hotlinks Tenor
///   preview thumbnails (the posted GIF itself is re-hosted same-origin).
/// - `connect-src` allows `ws:`/`wss:` for the `/ws` live hub.
/// - `frame-ancestors 'none'`, `object-src 'none'`, `base-uri 'self'` and
///   `form-action 'self'` are the real hardening wins this adds.
const CSP: HeaderValue = HeaderValue::from_static(
    "default-src 'self'; \
     base-uri 'self'; \
     object-src 'none'; \
     frame-ancestors 'none'; \
     img-src 'self' data: blob: https://tenor.com https://*.tenor.com; \
     media-src 'self' blob:; \
     font-src 'self' data:; \
     style-src 'self' 'unsafe-inline'; \
     script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
     connect-src 'self' ws: wss: blob:; \
     form-action 'self'",
);

/// Non-standard header name not in `axum::http::header`.
const PERMISSIONS_POLICY_NAME: HeaderName = HeaderName::from_static("permissions-policy");

/// Set the six security headers on every response. Applied as the outermost
/// router layer so redirects and error responses are covered too.
pub async fn set_security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers
        .entry(header::STRICT_TRANSPORT_SECURITY)
        .or_insert(HSTS);
    headers
        .entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(NOSNIFF);
    headers
        .entry(header::X_FRAME_OPTIONS)
        .or_insert(FRAME_OPTIONS);
    headers
        .entry(header::REFERRER_POLICY)
        .or_insert(REFERRER_POLICY);
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(CSP);
    headers
        .entry(PERMISSIONS_POLICY_NAME)
        .or_insert(PERMISSIONS_POLICY);
    resp
}
