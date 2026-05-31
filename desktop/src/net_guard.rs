//! LC-210: SSRF guard for the desktop self-updater's outbound HTTP.
//!
//! The self-updater fetches an operator-configured `LETS_CHAT_UPDATE_URL`
//! (manifest + binary download) via `ureq`. The operator sets the initial
//! URL deliberately, so the meaningful SSRF risk is **redirects**: a
//! compromised / MITM'd update endpoint (or a DNS/BGP redirect) could send
//! the updater to an internal or attacker-controlled host. The cure is
//! **per-hop validation**, not initial-URL-only.
//!
//! ## Why a manual redirect loop (not ureq's auto-follow)
//!
//! ureq follows redirects internally with a single agent, so an unguarded
//! initial fetch would mean unguarded hops - it can't express "the operator
//! chose the initial URL, but redirect targets must still be public." We set
//! `.redirects(0)` and follow hops ourselves, validating each one. This also
//! makes per-hop validation independent of any literal-IP fast-path in
//! ureq's resolver (we URL-validate every Location before fetching it).
//!
//! ## Two layers, mirroring the server's LC-152 `http_client.rs`
//!
//! 1. **URL-input validation** (`host_resolves_all_public`): resolve the
//!    host and reject if ANY address is non-public. `to_socket_addrs`
//!    returns the IP itself for literal-IP hosts, so this catches the
//!    literal-IP redirect target (`http://10.0.0.1/`) and gives a clean
//!    `NonPublicTarget` error with the offending hop.
//! 2. **`PublicOnlyResolver`** on the guarded agent: filters the resolution
//!    ureq actually connects on to public addresses only, closing the
//!    resolve-then-connect TOCTOU on hostnames.
//!
//! `is_globally_routable` is ported VERBATIM from `server/src/ssrf.rs` (the
//! source of truth). It is pure `std::net` with no external crates, so
//! duplicating ~70 lines is correct here; a workspace-shared crate is a
//! separate refactor, out of LC-210's scope.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Redirect cap, matching `server/src/http_client.rs::MAX_REDIRECTS`.
pub const MAX_REDIRECTS: usize = 3;

/// IP allowlist: globally routable unicast only. Ported verbatim from
/// `server/src/ssrf.rs::is_globally_routable` (source of truth). Stable Rust
/// does not expose `IpAddr::is_global`, so every non-public range is rejected
/// explicitly. Keep in sync with the server function if it changes.
pub fn is_globally_routable(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
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

/// Which hop a guard rejection fired on. The killer tests match on this so a
/// redirect rejection is provably the redirect hop, not the initial URL.
#[derive(Debug, PartialEq, Eq)]
pub enum Hop {
    Initial,
    Redirect,
}

/// Errors from the guarded fetch. Specific variants (not a bare string) so
/// tests assert the rejection fired in the FILTER, not via a generic
/// connect-failed (the LC-152 property-not-proxy lesson).
#[derive(Debug)]
pub enum GuardError {
    InvalidUrl(String),
    UnsupportedScheme(String),
    /// Host resolved to a non-public address (or failed to resolve). Carries
    /// the hop so a redirect-to-private is distinguishable from a bad initial.
    NonPublicTarget {
        hop: Hop,
        host: String,
    },
    MissingLocation,
    TooManyRedirects,
    HttpStatus(u16),
    Transport(String),
}

impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuardError::InvalidUrl(s) => write!(f, "invalid url: {s}"),
            GuardError::UnsupportedScheme(s) => write!(f, "unsupported url scheme: {s}"),
            GuardError::NonPublicTarget { hop, host } => write!(
                f,
                "{} target {host} resolves to a non-public address; refusing to connect",
                match hop {
                    Hop::Initial => "initial",
                    Hop::Redirect => "redirect",
                }
            ),
            GuardError::MissingLocation => write!(f, "redirect response had no Location header"),
            GuardError::TooManyRedirects => write!(f, "exceeded {MAX_REDIRECTS} redirects"),
            GuardError::HttpStatus(c) => write!(f, "HTTP {c}"),
            GuardError::Transport(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for GuardError {}

/// Injectable IP policy. Production passes `is_globally_routable`; tests inject
/// a policy that allows a fixture's loopback and/or records the IPs it sees.
pub type IpPolicy = Arc<dyn Fn(IpAddr) -> bool + Send + Sync>;

/// True iff the URL's host resolves AND every resolved address passes the
/// policy. All-or-nothing (a `[public, private]` answer is the classic
/// DNS-rebinding setup; refusing the whole answer leaves no gap), mirroring
/// the server's `host_resolves_public`.
fn host_resolves_all_public(url: &Url, policy: &IpPolicy) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let Some(port) = url.port_or_known_default() else {
        return false;
    };
    match (host, port).to_socket_addrs() {
        Ok(addrs) => {
            let mut any = false;
            for sa in addrs {
                any = true;
                if !policy(sa.ip()) {
                    return false;
                }
            }
            any
        }
        Err(_) => false,
    }
}

/// A `ureq::Resolver` (closure blanket impl) that filters resolution to
/// policy-passing addresses and errors if any are non-public. Closes the
/// resolve-then-connect TOCTOU on the guarded agent.
fn public_only_resolver(policy: IpPolicy) -> impl Fn(&str) -> std::io::Result<Vec<SocketAddr>> {
    move |netloc: &str| {
        let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        if addrs.is_empty() || addrs.iter().any(|sa| !policy(sa.ip())) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "resolved to a non-public address",
            ));
        }
        Ok(addrs)
    }
}

fn guarded_agent(policy: IpPolicy) -> ureq::Agent {
    ureq::builder()
        .redirects(0) // we follow + validate redirects manually
        .resolver(public_only_resolver(policy))
        .build()
}

fn plain_agent() -> ureq::Agent {
    ureq::builder().redirects(0).build()
}

/// Production entry point: fetch `raw_url` with per-hop SSRF validation.
/// `allow_private_initial` exempts ONLY the initial URL from the public-IP
/// filter (the operator's deliberate choice, e.g. a private internal update
/// mirror); redirect targets are ALWAYS validated.
pub fn guarded_get(
    raw_url: &str,
    allow_private_initial: bool,
    timeout: Duration,
) -> Result<ureq::Response, GuardError> {
    guarded_fetch_with(
        raw_url,
        allow_private_initial,
        timeout,
        Arc::new(is_globally_routable),
    )
}

fn guarded_fetch_with(
    raw_url: &str,
    allow_private_initial: bool,
    timeout: Duration,
    policy: IpPolicy,
) -> Result<ureq::Response, GuardError> {
    let mut url = Url::parse(raw_url).map_err(|e| GuardError::InvalidUrl(e.to_string()))?;
    let guarded = guarded_agent(policy.clone());
    let plain = plain_agent();

    for hop in 0..=MAX_REDIRECTS {
        // Scheme is always validated (rejects file://, gopher://, etc.).
        if !matches!(url.scheme(), "http" | "https") {
            return Err(GuardError::UnsupportedScheme(url.scheme().to_string()));
        }
        // full_guard = every hop except an opt-out initial URL.
        let full_guard = hop > 0 || !allow_private_initial;
        if full_guard && !host_resolves_all_public(&url, &policy) {
            return Err(GuardError::NonPublicTarget {
                hop: if hop == 0 {
                    Hop::Initial
                } else {
                    Hop::Redirect
                },
                host: url.host_str().unwrap_or("").to_string(),
            });
        }

        let agent = if full_guard { &guarded } else { &plain };
        let resp = match agent.request_url("GET", &url).timeout(timeout).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(GuardError::HttpStatus(code)),
            Err(ureq::Error::Transport(t)) => return Err(GuardError::Transport(t.to_string())),
        };

        // .redirects(0) returns a 3xx as Ok (ureq unit.rs:165); follow manually.
        if (300..400).contains(&resp.status()) {
            let loc = resp
                .header("Location")
                .ok_or(GuardError::MissingLocation)?
                .to_string();
            url = url
                .join(&loc)
                .map_err(|e| GuardError::InvalidUrl(e.to_string()))?;
            continue;
        }
        return Ok(resp);
    }
    Err(GuardError::TooManyRedirects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Mutex;

    // Test policy: allow a specific set of loopback addrs (the fixtures),
    // reject everything else non-public, and record every IP it is asked
    // about (for the per-hop-firing assertion).
    fn recording_policy(allowed: Vec<IpAddr>) -> (IpPolicy, Arc<Mutex<Vec<IpAddr>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let policy: IpPolicy = Arc::new(move |ip: IpAddr| {
            seen2.lock().unwrap().push(ip);
            if allowed.contains(&ip) {
                return true;
            }
            is_globally_routable(ip)
        });
        (policy, seen)
    }

    /// One-shot loopback server: accepts a single connection, sends `resp`,
    /// closes. Returns the bound addr. `resp` is the full HTTP/1.1 response.
    fn one_shot(bind: Ipv4Addr, resp: &'static str) -> SocketAddr {
        let listener = TcpListener::bind((bind, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            }
        });
        addr
    }

    // Test 1 (classify + literal-IP spike): the filter sees literal-IP
    // netlocs and rejects non-public ones; public IPs pass.
    #[test]
    fn policy_rejects_literal_private_and_metadata_ips() {
        assert!(!is_globally_routable("10.0.0.1".parse().unwrap()));
        assert!(!is_globally_routable("127.0.0.1".parse().unwrap()));
        assert!(!is_globally_routable("169.254.169.254".parse().unwrap())); // cloud metadata
        assert!(!is_globally_routable("100.64.0.1".parse().unwrap())); // CGNAT
        assert!(is_globally_routable("93.184.216.34".parse().unwrap())); // public
                                                                         // A literal-IP URL reaches the public-IP check (to_socket_addrs returns
                                                                         // the IP itself), so it is refused without a custom resolver.
        let policy: IpPolicy = Arc::new(is_globally_routable);
        let url = Url::parse("http://10.0.0.1/").unwrap();
        assert!(!host_resolves_all_public(&url, &policy));
    }

    // Test 2 (302 -> private refused, end-to-end): a redirect to a private IP
    // is rejected by the FILTER at the redirect hop, before any connect to it.
    #[test]
    fn redirect_to_private_ip_is_refused_at_redirect_hop() {
        let fixture = one_shot(
            Ipv4Addr::LOCALHOST,
            "HTTP/1.1 302 Found\r\nLocation: http://10.0.0.1/evil\r\nContent-Length: 0\r\n\r\n",
        );
        let (policy, _) = recording_policy(vec![fixture.ip()]); // allow the fixture only
        let err = guarded_fetch_with(
            &format!("http://{fixture}/"),
            false,
            Duration::from_secs(2),
            policy,
        )
        .expect_err("redirect to 10.0.0.1 must be refused");
        assert!(
            matches!(
                err,
                GuardError::NonPublicTarget {
                    hop: Hop::Redirect,
                    ..
                }
            ),
            "expected NonPublicTarget{{Redirect}}, got {err:?}"
        );
    }

    // Test 3 (per-hop firing): a 2-hop chain validates EACH hop's host, not
    // just the initial one. Distinct loopback IPs so the recorder can prove
    // the redirect target (127.0.0.2) was validated.
    #[test]
    fn each_redirect_hop_is_validated() {
        let b = one_shot(
            Ipv4Addr::new(127, 0, 0, 2),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi",
        );
        let a = one_shot(
            Ipv4Addr::LOCALHOST,
            // Location points at the second fixture on 127.0.0.2.
            Box::leak(
                format!("HTTP/1.1 302 Found\r\nLocation: http://{b}/\r\nContent-Length: 0\r\n\r\n")
                    .into_boxed_str(),
            ),
        );
        let (policy, seen) = recording_policy(vec![a.ip(), b.ip()]);
        let resp = guarded_fetch_with(
            &format!("http://{a}/"),
            false,
            Duration::from_secs(2),
            policy,
        )
        .expect("2-hop chain to allowed loopbacks should succeed");
        assert_eq!(resp.status(), 200);
        let seen = seen.lock().unwrap();
        assert!(
            seen.contains(&b.ip()),
            "redirect target {} must have been validated; saw {:?}",
            b.ip(),
            seen
        );
    }

    // Structural no-bypass ban (LC-152 shape): every outbound ureq
    // construction in desktop/src/ MUST go through net_guard, except the two
    // allow-listed files. net_guard.rs owns the guarded agents; welcome.rs is
    // the deliberately-unguarded LAN server probe (see its inline rationale).
    //
    // LC-210-GREPBAN-FULLPATH (#285): allow-list entries are full paths
    // RELATIVE TO src/ (forward-slash separated), NOT basenames - mirroring the
    // LC-152/204/206 bans, which chose full paths specifically so a future
    // same-basename file in a subdirectory cannot inherit the exemption (a
    // `src/foo/welcome.rs` is `foo/welcome.rs`, not `welcome.rs`). The walker
    // is recursive for the same reason: a raw ureq added in a future subdir
    // file must be SCANNED, not silently skipped as the old flat read_dir did.
    const BAN_ALLOWED: &[&str] = &["net_guard.rs", "welcome.rs"];
    const BANNED: &[&str] = &[
        "ureq::get(",
        "ureq::post(",
        "ureq::request(",
        "ureq::request_url(",
        "ureq::builder(",
        "ureq::agent(",
        "ureq::AgentBuilder",
    ];

    fn src_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// Every `.rs` file under src/, recursively.
    fn src_files() -> Vec<std::path::PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&src_root(), &mut out);
        out
    }

    /// Path relative to src/, forward-slash separated, so allow-list matching
    /// is stable across platforms and across subdirectories.
    fn rel_to_src(path: &std::path::Path) -> String {
        path.strip_prefix(src_root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    }

    #[test]
    fn no_raw_ureq_construction_outside_net_guard() {
        let mut violations = Vec::new();
        for path in src_files() {
            let rel = rel_to_src(&path);
            if BAN_ALLOWED.contains(&rel.as_str()) {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap();
            for (n, line) in body.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for pat in BANNED {
                    if line.contains(pat) {
                        violations.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "raw ureq construction outside net_guard - route through \
             net_guard::guarded_get:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn ban_allow_list_entries_are_load_bearing() {
        // Each allow-listed file must actually contain a banned token, so a
        // refactor that moves the raw ureq elsewhere fails HERE rather than
        // silently leaving the guard's coverage expanded over the LAN probe.
        // `allowed` is a src-relative path, so `src_root().join(allowed)`
        // resolves a future subdir entry too.
        for allowed in BAN_ALLOWED {
            let path = src_root().join(allowed);
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("allow-listed {allowed} not found; update the ban"));
            assert!(
                BANNED.iter().any(|pat| body.contains(pat)),
                "{allowed} is allow-listed but contains no raw ureq construction; \
                 the exemption is dead - remove it from BAN_ALLOWED"
            );
        }
    }

    // Test 4 (partial-bypass security property, refinement 1): with the
    // initial-URL opt-out ON, a PRIVATE initial URL is allowed, but a redirect
    // to a private IP is STILL refused. Kept separate from test 2 because it
    // proves the opt-out exempts only the initial hop, never the chain.
    #[test]
    fn opt_out_allows_private_initial_but_still_guards_redirects() {
        let fixture = one_shot(
            Ipv4Addr::LOCALHOST, // private initial URL, allowed only by the opt-out
            "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n",
        );
        // Policy does NOT pre-allow the fixture: the opt-out (not the policy)
        // is what lets the private initial URL through.
        let policy: IpPolicy = Arc::new(is_globally_routable);
        let err = guarded_fetch_with(
            &format!("http://{fixture}/"),
            true, // allow_private_initial
            Duration::from_secs(2),
            policy,
        )
        .expect_err("redirect to metadata IP must be refused even with opt-out");
        assert!(
            matches!(
                err,
                GuardError::NonPublicTarget {
                    hop: Hop::Redirect,
                    ..
                }
            ),
            "opt-out must still guard redirects; got {err:?}"
        );
    }
}
