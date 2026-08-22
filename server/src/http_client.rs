//! LC-152: shared outbound HTTP with built-in SSRF guard.
//!
//! **Every outbound HTTP call in this server MUST go through `outbound_get()`
//! or `outbound_post()`** below (redirect-following is the caller's job, per
//! the "No auto-redirect client" note below). The one deliberate exception is
//! `outbound_trusted_post()`, which targets OPERATOR-CONFIGURED endpoints
//! (LC-393 server-side STT) WITHOUT the public-IP filter so a localhost/
//! internal service is reachable - same trust posture as the SMTP/IMAP relays;
//! it must never see a user/remote-derived URL. It still lives in THIS module,
//! so the grep-ban stays a single-file allowance. The
//! underlying `reqwest::Client`s are private to this module - there is NO
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
//!    itself for literal-IP URLs - so this layer catches the
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
//! - same security boundary, fewer moving parts.
//!
//! ## Pool keying (confirmed; no design impact)
//!
//! reqwest 0.12 keys the connection pool on `(scheme, authority)` where
//! authority is the hostname + port - NOT the resolved IP. A validated
//! connection cannot be reused for a different host; Host-header
//! manipulation on a pooled HTTPS connection is bounded by the original
//! SNI'd hostname.
//!
//! ## No auto-redirect client (LC-152-TOCTOU, #286)
//!
//! There is deliberately NO reqwest-auto-redirect client here. reqwest's
//! redirect follower resolves each new hop with its OWN connector, and the
//! connector's literal-IP fast path skips `dns::Resolve` (see layer 1) - so a
//! response that 302s to a literal private IP (`http://169.254.169.254/...`)
//! would be followed WITHOUT either layer firing, an SSRF bypass. The only
//! code that follows redirects (link unfurl) does it MANUALLY, re-running
//! `outbound_get` (and thus full URL-input validation) on every hop, exactly
//! like the desktop `net_guard` manual loop (LC-210). Any future redirect-
//! following caller MUST use that per-hop-validated pattern, never reqwest's
//! `Policy::limited`.
//!
//! ## On "pinning to the validated IP" (#286)
//!
//! For a hostname, `PublicOnlyResolver` IS the authoritative resolution
//! reqwest connects on, and it validates public-only on exactly the addresses
//! it returns (`lc152_resolver_property_pair::reqwest_connects_to_resolver_supplied_ip`
//! proves reqwest uses the resolver's IPs, not a silent re-resolve). So the
//! connected IP is always a validated-public IP, and a DNS rebind to a private
//! address between submit-time validation and connect-time resolution is
//! REFUSED at connect - the rebind window is already collapsed, not left open.
//! A single connect-layer IP-pin (Option A below) would additionally guarantee
//! "connected IP == the exact IP validated" even against a hypothetical
//! `is_globally_routable` bug; that is defense-in-depth only and still awaits a
//! clean reqwest connector hook.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Default client timeout. Per-request override via `.timeout()` on the
/// `RequestBuilder` is honored.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors surfaced from the URL-input validation layer. Each variant
/// corresponds to a distinct attack class so callers can pattern-match if
/// they need to (the killer test pair asserts on the specific variant -
/// `is_err()` alone is too loose, because a connect-failed / network-
/// unreachable also satisfies it without proving the filter fired).
#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    /// The URL string failed to parse.
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// The URL parsed but its scheme is not `http` or `https`. Catches
    /// `file://`, `gopher://`, `chrome://`, etc. - none of which should
    /// reach reqwest from operator- or user-supplied URLs.
    #[error("unsupported url scheme: {0}")]
    UnsupportedScheme(String),

    /// The URL's host either failed to resolve OR resolved to a
    /// non-public address (private, loopback, link-local, broadcast,
    /// CGNAT, etc.). For literal-IP URLs the "resolution" returns the
    /// IP itself; this variant catches both literal-IP and DNS-resolved-
    /// to-private cases. This is the variant the killer test pair
    /// asserts on; do NOT bare-match `is_err()` - a connect-failed
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

/// Resolve `host` and return its addresses ONLY if every one is publicly
/// routable; otherwise `Err`. This is the connect-time guard: the addresses it
/// returns are exactly the addresses reqwest connects on, so the connected IP
/// is provably one this function validated (no second, unguarded resolution).
/// All-or-nothing: any non-public address rejects the WHOLE answer (a
/// dual-record `[public, private]` reply is the classic DNS-rebinding setup).
/// Factored out of the `Resolve` impl so it is unit-testable without
/// constructing a `reqwest::dns::Name`.
async fn resolve_public_only(host: &str) -> Result<Vec<SocketAddr>, String> {
    // Per `Resolve::resolve` contract: port 0 here is replaced by reqwest at
    // connect time with the URL's actual port.
    let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|e| e.to_string())?
        .collect();
    if resolved.is_empty() {
        return Err("no addresses returned for host".into());
    }
    for sa in &resolved {
        if !crate::ssrf::is_globally_routable(sa.ip()) {
            // Name "non-public" without naming the specific address so the
            // failure does not enumerate internal topology in logs.
            return Err(format!(
                "host {host} resolves to a non-public address; refusing"
            ));
        }
    }
    Ok(resolved)
}

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            match resolve_public_only(&host).await {
                Ok(addrs) => Ok(Box::new(addrs.into_iter()) as Addrs),
                Err(msg) => Err(msg.into()),
            }
        })
    }
}

fn resolver() -> Arc<PublicOnlyResolver> {
    static R: OnceLock<Arc<PublicOnlyResolver>> = OnceLock::new();
    R.get_or_init(|| Arc::new(PublicOnlyResolver)).clone()
}

/// Private. The pool-shared client. Redirects are disabled (`Policy::none`):
/// see the module doc - redirect-following must be per-hop validated by the
/// caller, never delegated to reqwest. Callers reach it only through
/// `outbound_get` / `outbound_post`, both of which do the URL-input validation
/// before handing back a `RequestBuilder`.
fn client_no_redirects() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("lets-chat/1")
            .timeout(DEFAULT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(resolver())
            .build()
            .expect("outbound client build (panic indicates a bug in the helper itself, not a runtime failure)")
    })
}

/// Validate a URL string before letting reqwest see it. Parses, gates
/// scheme to `http`/`https`, calls `ssrf::host_resolves_public` (which
/// resolves through `tokio::net::lookup_host` - handles literal-IP URLs
/// by returning the IP itself, so literal-private bypasses are caught
/// here, NOT at the resolver layer).
async fn validate_url(url: &str) -> Result<url::Url, OutboundError> {
    let parsed = url::Url::parse(url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(OutboundError::UnsupportedScheme(
            parsed.scheme().to_string(),
        ));
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

/// Pooled client for OPERATOR-CONFIGURED, TRUSTED endpoints (the SMTP / IMAP
/// class): NO public-IP SSRF filter, so it can reach a `localhost`/internal
/// service the operator deliberately points us at. Redirects still disabled.
fn client_trusted() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("lets-chat/1")
            .timeout(DEFAULT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("trusted outbound client build")
    })
}

/// Issue a POST to an OPERATOR-CONFIGURED, TRUSTED endpoint, bypassing the
/// public-IP SSRF filter so it can reach an internal/localhost service the
/// operator deliberately configured (LC-393 server-side STT, `stt::SttConfig`).
/// Scheme is still gated to http/https. This is the ONLY blessed un-filtered
/// HTTP path; it lives HERE, in the chokepoint module, so the LC-152 grep-ban
/// stays a single-file allowance. **Never** pass a URL derived from user or
/// remote input - only operator env config (same trust posture as the SMTP /
/// IMAP relays, which already connect to operator endpoints without an SSRF
/// filter).
pub async fn outbound_trusted_post(url: &str) -> Result<reqwest::RequestBuilder, OutboundError> {
    let parsed = url::Url::parse(url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(OutboundError::UnsupportedScheme(
            parsed.scheme().to_string(),
        ));
    }
    Ok(client_trusted().post(parsed))
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

#[cfg(test)]
mod tests {
    use super::*;

    // LC-152-TOCTOU (#286): the connect-time resolver is the authoritative
    // guard - it returns ONLY validated-public addresses, so the address
    // reqwest connects on is provably one this layer accepted. These exercise
    // `resolve_public_only` directly (the core the `Resolve` impl delegates to)
    // to prove the second layer refuses private resolutions on its own, not
    // just `validate_url`. Cases are deterministic: literal IPs never hit DNS,
    // and `localhost` is a hosts-file loopback.

    #[tokio::test]
    async fn resolver_refuses_loopback_literal() {
        assert!(resolve_public_only("127.0.0.1").await.is_err());
    }

    #[tokio::test]
    async fn resolver_refuses_rfc1918_and_metadata_literals() {
        assert!(resolve_public_only("10.0.0.1").await.is_err());
        assert!(resolve_public_only("169.254.169.254").await.is_err());
    }

    #[tokio::test]
    async fn resolver_refuses_localhost_hostname() {
        // A hostname that resolves to loopback must be refused at the
        // connect-time layer (this is the DNS-rebind-to-private case: the
        // resolution reqwest would connect on is rejected).
        assert!(resolve_public_only("localhost").await.is_err());
    }

    #[tokio::test]
    async fn resolver_accepts_public_literal() {
        // Control: a public literal passes and is returned for connection, so
        // the filter is not refusing everything. 93.184.216.34 (example.com)
        // is a stable documentation-adjacent public address; as a literal it
        // does not depend on DNS.
        let addrs = resolve_public_only("93.184.216.34")
            .await
            .expect("public literal must pass the filter");
        assert!(addrs
            .iter()
            .all(|sa| crate::ssrf::is_globally_routable(sa.ip())));
    }
}
