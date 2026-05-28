//! LC-152: shared outbound HTTP client with built-in SSRF guard.
//!
//! **Every outbound reqwest call in this server MUST go through one of the
//! `outbound_*()` constructors below.** A single bypass is a complete
//! SSRF: the audit that motivated this module found exactly that — the
//! Web Push delivery path at `push/mod.rs` had no SSRF guard at all
//! because nothing prevented constructing a raw `reqwest::Client`. The
//! bypass-prevention companion is the grep-ban test
//! `no_direct_reqwest_construction_in_src`; together they make the unsafe
//! path not-exist rather than audited-absent.
//!
//! ## Mechanism (closes the LC-152 TOCTOU)
//!
//! A custom `dns::Resolve` (`PublicOnlyResolver`) filters every resolution
//! to publicly-routable IPs INSIDE reqwest's own resolution path. The
//! check and the connect are the same resolution, so the
//! check-then-reqwest-reresolves window that motivated this PR is
//! collapsed to zero, not narrowed. If any returned IP is non-public the
//! whole resolution is refused — matches `ssrf::host_resolves_public`'s
//! pre-existing all-or-nothing semantics so a dual-record
//! `[public, private]` answer is rejected, not partially trusted.
//!
//! ## Pool keying (confirmed; no design impact)
//!
//! reqwest 0.12 keys the connection pool on `(scheme, authority)` where
//! authority is the hostname + port — NOT the resolved IP. A validated
//! connection cannot be reused for a different host; Host-header
//! manipulation on a pooled HTTPS connection is bounded by the original
//! SNI'd hostname. Documented here for the threat model.
//!
//! ## Redirect cap (independent of per-hop filter)
//!
//! The cap and the filter are independent defences and both run on every
//! redirect. The filter catches a single hop to an internal host (the
//! resolver is invoked once per redirect connection); the cap bounds
//! chain length so a redirect-loop can't be used to amplify outbound
//! traffic. `outbound_following_redirects()` uses `Policy::limited(3)` to
//! match the unfurl path's pre-existing cap; `outbound_no_redirects()` is
//! the default for every other site.
//!
//! ## No `// allow:` escape hatch
//!
//! Adding a per-site exemption to the grep-ban (even for a "fixed
//! endpoint" like Web Push gateways) creates the bypass the ban exists
//! to prevent — a future PR copies the annotation onto a non-fixed-
//! endpoint site and reintroduces the unguarded path. The exception-free
//! rule is enforceable; rule-with-one-blessed-exception grows. Push
//! endpoints resolve public for every browser-registered destination
//! (Firefox, Chrome, Safari push gateways are all on public IPs); the
//! filter never rejects them, zero behavior change. If a future
//! deployment legitimately needs to reach a private host (operator's
//! self-hosted Unified Push gateway on an internal network), the fix is
//! an env-var allowlist read by `PublicOnlyResolver`, not a code-path
//! exemption.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Maximum redirect hops for paths that legitimately follow redirects
/// (currently only the unfurl preview). Matches the pre-existing
/// `routes::unfurl` cap.
const MAX_REDIRECTS: usize = 3;

/// Default client timeout. Per-request override via `.timeout()` on the
/// `RequestBuilder` is honored; sites that need a tighter bound (e.g.,
/// LC-78 bridge avatar fetch caps at 5s) set their own per request.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// LC-152: refuse every resolution where any address is non-public.
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // Per `Resolve::resolve` contract: port 0 here is replaced by
            // reqwest at connect time with the URL's actual port. We only
            // care about the resolved IPs.
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
                    // specific address, so the error doesn't enumerate
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

/// The default outbound client. Refuses to follow redirects: a 3xx to an
/// internal host is a known SSRF-via-redirect amplification path, and the
/// only site in this server with a legitimate need to follow redirects is
/// link-unfurl (use `outbound_following_redirects` there).
///
/// Returned as a `&'static reqwest::Client` so the connection pool is
/// shared across every caller — important for the LC-75 delivery loop
/// which can issue many requests per tick.
pub fn outbound_no_redirects() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_client(reqwest::redirect::Policy::none()))
}

/// Outbound client that follows up to `MAX_REDIRECTS` redirects. The
/// per-hop filter applies on each redirect (the resolver is invoked per
/// connection, and a redirect to a new host opens a new connection). Use
/// only for link-unfurl, which legitimately needs to chase short redirect
/// chains for canonical-URL discovery.
pub fn outbound_following_redirects() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_client(reqwest::redirect::Policy::limited(MAX_REDIRECTS)))
}

/// LC-152 test seam. Returns a fresh client with the DEFAULT reqwest
/// resolver (no public-IP filter) so tests can target loopback receivers.
/// Mirrors the LC-75 `run_delivery_tick_unchecked` + LC-78
/// `fetch_and_cache_unchecked` convention: production code MUST NOT call
/// this. The grep-ban test skips this module, so the unchecked path is
/// not auto-callable from elsewhere; calling it from `server/src/`
/// outside this module would compile but is by-policy forbidden.
#[doc(hidden)]
pub fn outbound_unchecked() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat/test")
        .timeout(DEFAULT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("test client build")
}
