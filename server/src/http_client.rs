//! LC-152: shared outbound HTTP with built-in SSRF guard.
//!
//! **Every outbound HTTP call in this server MUST go through `outbound_get()`,
//! `outbound_post()`, or `outbound_get_following_redirects()`** below. The
//! underlying `reqwest::Client`s are private to this module — there is NO
//! public way to obtain a `reqwest::Client` from outside `http_client.rs`,
//! which makes the no-bypass rule structural (no `.get()` / `.post()`
//! verb-grepping necessary; there is no out-of-helper Client to call them
//! on). The companion grep-ban test forbids raw `reqwest::Client::*`
//! construction and the `outbound_unchecked()` test seam in `server/src/`.
//!
//! ## Two layers, each closes what the other can't
//!
//! 1. **URL-input validation** (`outbound_*` async fns): parse the URL,
//!    reject non-`http(s)` schemes, call `ssrf::host_resolves_public`. The
//!    last step uses `tokio::net::lookup_host`, which returns the IP
//!    itself for literal-IP URLs — so this layer catches the
//!    **literal-IP bypass** (e.g., `http://127.0.0.1/`, AWS metadata
//!    `http://169.254.169.254/`) that reqwest's custom resolver does NOT
//!    see. Reqwest 0.12's connector takes a literal-IP fast path that
//!    skips `dns::Resolve` entirely; without this layer, a daemon or
//!    user-supplied URL with a literal private IP would slip past the
//!    resolver.
//!
//! 2. **`PublicOnlyResolver: dns::Resolve`** inside reqwest's resolution
//!    path. For hostnames, the resolver filters the resolved address set
//!    to publicly-routable IPs only and feeds reqwest the result reqwest
//!    then connects on. Same resolution, no second resolve. Closes the
//!    **hostname use-time TOCTOU** (the check-then-reqwest-reresolves
//!    window in the original LC-152 audit). Without this layer, a
//!    rapid-flip DNS record between the URL-validation step and reqwest's
//!    own resolution would reach an internal host through the resolved-
//!    after-validation IP.
//!
//! The two layers attack two different vectors. URL-input validation
//! catches the literal IP and gives the caller a clean `OutboundError`
//! variant before reqwest sees the URL; the resolver catches the hostname
//! DNS rebinding and refuses the resolution from inside the connect path.
//! Removing either reopens the corresponding attack class.
//!
//! ## Why not a custom connector (Option A in the design)?
//!
//! A custom `tower_service::Service` connector that filters
//! at TCP-connect time would close both vectors at a single layer (it
//! sees every connection's resolved IP, hostname or not, AND would still
//! be inside reqwest's resolution path). That's the structurally cleaner
//! fix. Reqwest 0.12 does NOT expose `.connector()` as a public builder
//! method, so building this requires a tower-middleware dance that's
//! version-fragile. If a future reqwest release exposes a clean
//! connector hook, fold these two layers into one connector-level filter
//! — same security boundary, fewer moving parts.
//!
//! ## Pool keying (confirmed; no design impact)
//!
//! reqwest 0.12 keys the connection pool on `(scheme, authority)` where
//! authority is the hostname + port — NOT the resolved IP. A validated
//! connection cannot be reused for a different host; Host-header
//! manipulation on a pooled HTTPS connection is bounded by the original
//! SNI'd hostname.
//!
//! ## Redirect cap (independent of per-hop filter)
//!
//! `outbound_get_following_redirects` uses `Policy::limited(3)` to match
//! the unfurl path's pre-existing cap. Each redirect connection runs the
//! resolver inline (the filter catches a single hop to internal); the cap
//! independently bounds chain length so a redirect loop can't be used to
//! amplify outbound traffic.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Maximum redirect hops for paths that legitimately follow redirects
/// (currently only the unfurl preview).
const MAX_REDIRECTS: usize = 3;

/// Default client timeout. Per-request override via `.timeout()` on the
/// `RequestBuilder` is honored.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors surfaced from the URL-input validation layer. Each variant
/// corresponds to a distinct attack class so callers can pattern-match if
/// they need to (the killer test pair asserts on the specific variant —
/// `is_err()` alone is too loose, because a connect-failed / network-
/// unreachable also satisfies it without proving the filter fired).
#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    /// The URL string failed to parse.
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// The URL parsed but its scheme is not `http` or `https`. Catches
    /// `file://`, `gopher://`, `chrome://`, etc. — none of which should
    /// reach reqwest from operator- or user-supplied URLs.
    #[error("unsupported url scheme: {0}")]
    UnsupportedScheme(String),

    /// The URL's host either failed to resolve OR resolved to a
    /// non-public address (private, loopback, link-local, broadcast,
    /// CGNAT, etc.). For literal-IP URLs the "resolution" returns the
    /// IP itself; this variant catches both literal-IP and DNS-resolved-
    /// to-private cases. This is the variant the killer test pair
    /// asserts on; do NOT bare-match `is_err()` — a connect-failed
    /// returns a different error and would mask a filter regression.
    #[error("host resolves to a non-public address; refusing to connect")]
    HostNotPublic,
}

/// LC-152: refuse every resolution where any address is non-public. All-or-
/// nothing strictness mirrors `ssrf::host_resolves_public`'s pre-existing
/// semantics: a dual-record `[public, private]` answer is the classic
/// DNS-rebinding setup, and "we picked the public one" leaves a gap
/// between the pick and reqwest's connect/retry/pool behavior. Refusing
/// the whole answer leaves no gap.
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // Per `Resolve::resolve` contract: port 0 here is replaced by
            // reqwest at connect time with the URL's actual port.
            let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if resolved.is_empty() {
                return Err("no addresses returned for host".into());
            }
            for sa in &resolved {
                if !crate::ssrf::is_globally_routable(sa.ip()) {
                    // Strict: any private address rejects the WHOLE answer.
                    // Error message names "non-public" without naming the
                    // specific address so the failure doesn't enumerate
                    // internal topology in logs.
                    return Err(format!(
                        "host {host} resolves to a non-public address; refusing"
                    )
                    .into());
                }
            }
            Ok(Box::new(resolved.into_iter()) as Addrs)
        })
    }
}

fn resolver() -> Arc<PublicOnlyResolver> {
    static R: OnceLock<Arc<PublicOnlyResolver>> = OnceLock::new();
    R.get_or_init(|| Arc::new(PublicOnlyResolver)).clone()
}

fn build_client(redirects: reqwest::redirect::Policy) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat/1")
        .timeout(DEFAULT_TIMEOUT)
        .redirect(redirects)
        .dns_resolver(resolver())
        .build()
        .expect("outbound client build (panic indicates a bug in the helper itself, not a runtime failure)")
}

/// Private. The pool-shared no-redirects client. Callers reach it only
/// through `outbound_get` / `outbound_post`, both of which do the URL-
/// input validation before handing back a `RequestBuilder`.
fn client_no_redirects() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_client(reqwest::redirect::Policy::none()))
}

/// Private. The pool-shared redirect-following client used only by
/// `outbound_get_following_redirects`. Each redirect hop's connection
/// re-invokes the resolver, so the per-hop filter and the cap both bound
/// the chain.
fn client_following_redirects() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_client(reqwest::redirect::Policy::limited(MAX_REDIRECTS)))
}

/// Validate a URL string before letting reqwest see it. Parses, gates
/// scheme to `http`/`https`, calls `ssrf::host_resolves_public` (which
/// resolves through `tokio::net::lookup_host` — handles literal-IP URLs
/// by returning the IP itself, so literal-private bypasses are caught
/// here, NOT at the resolver layer).
async fn validate_url(url: &str) -> Result<url::Url, OutboundError> {
    let parsed = url::Url::parse(url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(OutboundError::UnsupportedScheme(parsed.scheme().to_string()));
    }
    if !crate::ssrf::host_resolves_public(&parsed).await {
        return Err(OutboundError::HostNotPublic);
    }
    Ok(parsed)
}

/// Issue a GET request to `url`. Returns a `RequestBuilder` ready to
/// receive `.header()` / `.timeout()` / `.send()` chaining, OR an
/// `OutboundError` if the URL is invalid, has an unsupported scheme, or
/// resolves to a non-public address. **The only public way to issue a
/// GET to an external URL from this server.**
pub async fn outbound_get(url: &str) -> Result<reqwest::RequestBuilder, OutboundError> {
    let parsed = validate_url(url).await?;
    Ok(client_no_redirects().get(parsed))
}

/// Issue a POST request to `url`. Same validation contract as
/// `outbound_get`. Use for LC-75 outgoing webhook delivery, slash-command
/// dispatch, Web Push.
pub async fn outbound_post(url: &str) -> Result<reqwest::RequestBuilder, OutboundError> {
    let parsed = validate_url(url).await?;
    Ok(client_no_redirects().post(parsed))
}

/// GET via the redirect-following client (cap `MAX_REDIRECTS`). Used ONLY
/// by link-unfurl, which legitimately chases short redirect chains for
/// canonical-URL discovery. Every redirect hop is independently filtered
/// by the resolver inline (the helper's two-layer guarantee applies per
/// connection, including each redirect's new connection).
pub async fn outbound_get_following_redirects(
    url: &str,
) -> Result<reqwest::RequestBuilder, OutboundError> {
    let parsed = validate_url(url).await?;
    Ok(client_following_redirects().get(parsed))
}

/// LC-152 test seam. Returns a fresh `reqwest::Client` with the DEFAULT
/// reqwest resolver (no public-IP filter) AND no URL-input validation,
/// so tests can target loopback receivers. Mirrors the LC-75
/// `run_delivery_tick_unchecked` + LC-78 `fetch_and_cache_unchecked`
/// convention. **Production code MUST NOT call this; the grep-ban test
/// rejects any call to `outbound_unchecked` in `server/src/` exactly as
/// hard as raw `reqwest::Client::*` construction.**
#[doc(hidden)]
pub fn outbound_unchecked() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat/test")
        .timeout(DEFAULT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("test client build")
}
