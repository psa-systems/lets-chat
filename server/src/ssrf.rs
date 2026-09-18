//! Shared SSRF guard: reject outbound fetches whose target resolves to a
//! non-globally-routable address. Used by the link unfurler (LC-150) and the
//! outgoing / slash-command webhook deliverers (LC-152) so the IP allowlist is
//! defined once and every fetch path enforces the same policy.
//!
//! DNS rebinding is closed at connect time, not merely mitigated: the shared
//! outbound client (`http_client`) resolves through a public-only
//! `dns::Resolve` whose returned addresses ARE the addresses reqwest connects
//! on, so a rebind to a private address between this submit-time check and the
//! connect is refused at connect (LC-152-TOCTOU, #286). Literal-IP URLs are
//! rejected at submit (the connector's literal-IP fast path skips the
//! resolver), and redirect-following re-runs the full check on every hop (see
//! `unfurl`).

use url::Url;

/// IP allowlist: globally routable unicast only. Defined in the shared
/// `ip-policy` crate so the desktop self-updater's guard
/// (`desktop/src/net_guard.rs`) depends on the same implementation instead of
/// a hand-synced copy.
pub use ip_policy::is_globally_routable;

/// Resolve a URL's host and require that EVERY resolved address is globally
/// routable. Returns false on no host, DNS failure, an empty address set, or
/// any non-public address (so a dual-record `[public, private]` answer is
/// rejected). Call before connecting, and again for each redirect hop.
pub async fn host_resolves_public(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let Ok(addrs) = tokio::net::lookup_host((host, port)).await else {
        return false;
    };
    let mut any_addr = false;
    for sa in addrs {
        any_addr = true;
        if !is_globally_routable(sa.ip()) {
            return false;
        }
    }
    any_addr
}

#[cfg(test)]
mod tests {
    use super::*;

    // `is_globally_routable` itself is tested in the `ip-policy` crate, its
    // single source of truth; these tests cover this module's own
    // DNS-resolution wrapper.

    #[tokio::test]
    async fn host_resolves_public_rejects_ip_literals_in_private_ranges() {
        for u in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://169.254.169.254/",
            "http://[::1]/",
            "http://[fc00::1]/",
        ] {
            let url = Url::parse(u).unwrap();
            assert!(
                !host_resolves_public(&url).await,
                "{u} must be rejected by the SSRF guard"
            );
        }
    }
}
