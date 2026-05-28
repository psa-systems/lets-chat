//! LC-152: killer test pair — destination correctness + rejection correctness.
//!
//! The whole LC-152 fix rests on TWO contracts, each closing a distinct
//! attack class:
//!
//! 1. **Reqwest connects to the IP `dns::Resolve` returned.** Without
//!    this, the `PublicOnlyResolver` is theatrical. Tested directly:
//!    bind a real listener at a known IP, return that IP from a stub
//!    resolver, GET, assert the LISTENER RECEIVED THE REQUEST (arrival,
//!    not resolve-count).
//!
//! 2. **The URL-input layer (`outbound_get` / `outbound_post`) rejects
//!    every non-public destination, including literal IPs.** Without
//!    this, literal-IP URLs skip `dns::Resolve` entirely (reqwest takes
//!    a literal-IP fast path) and the resolver never fires. Tested
//!    directly: call `outbound_get` with a literal-private URL and
//!    assert the **specific** `OutboundError::HostNotPublic` variant —
//!    `is_err()` alone is too loose (a connect-failed or network-
//!    unreachable also satisfies it without proving the filter fired,
//!    as the prior round caught).
//!
//! Together: public-hostname resolution arrives at the resolver's IP
//! (TOCTOU window collapsed by reqwest's contract); literal-IP URLs are
//! refused at the API boundary before reqwest sees them; hostname-
//! resolves-private URLs are refused at the same API boundary AND by the
//! resolver if a hostname-only test reaches the resolver.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use lets_chat::http_client::{self, OutboundError};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::net::TcpListener;

// ────────────────────────────────────────────────────────────────────
// Test 1: destination correctness (the resolver-honoring contract)
// ────────────────────────────────────────────────────────────────────

/// Counter-instrumented stub resolver. Returns a fixed `SocketAddr` for
/// every hostname. The test asserts the LISTENER bound to that address
/// received the request — proving reqwest used the resolver's IP at
/// connect time, did not silently re-resolve, did not look up the fake
/// hostname via the system resolver.
struct StubResolver {
    target: SocketAddr,
    calls: Arc<AtomicUsize>,
}

impl Resolve for StubResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        let target = self.target;
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(std::iter::once(target)) as Addrs)
        })
    }
}

#[tokio::test]
async fn reqwest_connects_to_resolver_supplied_ip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let listener_addr = listener.local_addr().unwrap();

    #[derive(Clone)]
    struct Shared {
        hits: Arc<AtomicUsize>,
    }
    let hits = Arc::new(AtomicUsize::new(0));
    let state = Shared { hits: hits.clone() };
    let app = Router::new()
        .route(
            "/ping",
            get(|State(s): State<Shared>| async move {
                s.hits.fetch_add(1, Ordering::SeqCst);
                "pong"
            }),
        )
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = Arc::new(StubResolver {
        target: listener_addr,
        calls: calls.clone(),
    });
    // This is a test-only client built with a stub resolver to prove the
    // reqwest-honors-Resolve contract. NOT the production helper (which
    // would reject 127.0.0.1 as non-public). Direct construction is
    // permitted only in `tests/`.
    let client = reqwest::Client::builder()
        .dns_resolver(resolver)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // `test.invalid` is RFC 2606 reserved and never resolves via the
    // system resolver. If reqwest silently bypassed the custom resolver,
    // this request would fail at DNS lookup. Success proves the stub's
    // IP was used.
    let port = listener_addr.port();
    let resp = client
        .get(format!("http://test.invalid:{port}/ping"))
        .send()
        .await
        .expect("request must succeed via the stub resolver's pinned IP");
    assert!(resp.status().is_success(), "status: {}", resp.status());
    assert_eq!(resp.text().await.unwrap(), "pong");
    // ARRIVAL assertion. The listener saw the request, proving the
    // connection went to listener_addr (the resolver's output), not to
    // a re-resolved address.
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "listener at the resolver's IP must have received the request"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "stub resolver must have been called",
    );
}

// ────────────────────────────────────────────────────────────────────
// Test 2-5: rejection correctness — assert the SPECIFIC variant
//           Bare is_err() is too loose; a connect-failed or
//           network-unreachable satisfies it without proving the filter
//           fired.
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn outbound_get_rejects_literal_loopback_with_host_not_public() {
    // The literal-IP bypass that the URL-input layer specifically closes.
    // The reqwest resolver would never have fired here (literal IPs skip
    // `dns::Resolve`), so this test fails if we ever regress and rely on
    // the resolver alone.
    let result = http_client::outbound_get("http://127.0.0.1/x").await;
    assert!(
        matches!(result, Err(OutboundError::HostNotPublic)),
        "expected OutboundError::HostNotPublic; got: {:?}",
        result.as_ref().err()
    );
}

#[tokio::test]
async fn outbound_get_rejects_literal_rfc1918_with_host_not_public() {
    // RFC 1918 private space. Same fast-path bypass without the URL-input
    // layer; this asserts the filter catches it on the literal-IP path.
    let result = http_client::outbound_get("http://10.0.0.1/x").await;
    assert!(
        matches!(result, Err(OutboundError::HostNotPublic)),
        "expected HostNotPublic; got: {:?}",
        result.as_ref().err()
    );
}

#[tokio::test]
async fn outbound_get_rejects_cloud_metadata_endpoint_with_host_not_public() {
    // The audit's specific concern: `169.254.169.254` is the AWS / GCP /
    // Azure instance-metadata endpoint. If Web Push had remained on the
    // raw client, a malicious push-subscription endpoint pointing here
    // would have been reachable. This assertion proves the URL-input
    // layer refuses it via the specific variant — not via a "network
    // unreachable" false positive.
    let result = http_client::outbound_get("http://169.254.169.254/latest/meta-data/").await;
    assert!(
        matches!(result, Err(OutboundError::HostNotPublic)),
        "metadata endpoint must be refused via HostNotPublic; got: {:?}",
        result.as_ref().err()
    );
}

#[tokio::test]
async fn outbound_post_rejects_literal_private_too() {
    // Symmetric assertion: POST and GET share the same URL-input layer.
    // If a refactor accidentally split them, this catches it.
    let result = http_client::outbound_post("http://192.168.1.1/x").await;
    assert!(
        matches!(result, Err(OutboundError::HostNotPublic)),
        "expected HostNotPublic on POST; got: {:?}",
        result.as_ref().err()
    );
}

#[tokio::test]
async fn outbound_get_rejects_non_http_scheme_with_unsupported_scheme() {
    // The OTHER class of bypass the URL-input layer catches: a non-
    // http(s) scheme that would never trigger DNS resolution at all.
    // A `file://` URL with a literal IP would slip past an IP-only filter
    // entirely if scheme were not gated separately.
    let result = http_client::outbound_get("file:///etc/passwd").await;
    assert!(
        matches!(result, Err(OutboundError::UnsupportedScheme(_))),
        "expected UnsupportedScheme; got: {:?}",
        result.as_ref().err()
    );
}

#[tokio::test]
async fn outbound_get_rejects_unparseable_url_with_invalid_url() {
    let result = http_client::outbound_get("not a url at all").await;
    assert!(
        matches!(result, Err(OutboundError::InvalidUrl(_))),
        "expected InvalidUrl; got: {:?}",
        result.as_ref().err()
    );
}

// ────────────────────────────────────────────────────────────────────
// Test 7: rejection happens at the URL layer, BEFORE any TCP connect
//         (fast-fail is what makes the literal-IP layer meaningful;
//         relying on a connect timeout is the math.rs-@bob@bob trap)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn rejection_is_fast_no_tcp_attempt() {
    // The URL-input layer must REJECT BEFORE any connect attempt. If it
    // didn't, the resolver's filter (which only fires for hostnames)
    // wouldn't catch literal-IP URLs and reqwest would attempt a TCP
    // connect to e.g. 10.0.0.1, potentially eating the default timeout
    // before failing. Asserting elapsed-time bounds catches a regression
    // where the URL-input layer is accidentally removed or bypassed.
    let started = std::time::Instant::now();
    let result = http_client::outbound_get("http://10.0.0.1:1/never").await;
    let elapsed = started.elapsed();
    assert!(
        matches!(result, Err(OutboundError::HostNotPublic)),
        "expected HostNotPublic; got: {:?}",
        result.as_ref().err()
    );
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "URL-input layer must reject without a connect attempt; elapsed: {elapsed:?}"
    );
}
