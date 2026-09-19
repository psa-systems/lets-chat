//! Shared IP allowlist policy for outbound-HTTP SSRF guards.
//!
//! `is_globally_routable` is the single source of truth for "is this address
//! safe to connect to" and is depended on by both `server`'s outbound-HTTP
//! guard (`server/src/ssrf.rs`) and the desktop self-updater's guard
//! (`desktop/src/net_guard.rs`), so the two copies can never drift again.
//! Pure `std::net`, no dependencies, so it stays a single tiny crate rather
//! than pulling any dependency tree into the desktop GUI binary.

use std::net::IpAddr;

/// IP allowlist: globally routable unicast only. Rejects loopback, private,
/// link-local, CGNAT, multicast, broadcast, unspecified, documentation, and
/// reserved addresses. Stable Rust does not yet expose `IpAddr::is_global`, so
/// we reject every non-public range we know about explicitly.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    fn v4(s: &str) -> IpAddr {
        s.parse::<std::net::Ipv4Addr>().unwrap().into()
    }
    fn v6(s: &str) -> IpAddr {
        s.parse::<Ipv6Addr>().unwrap().into()
    }

    #[test]
    fn rejects_non_public_v4() {
        for s in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.10.10",
            "100.64.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "0.0.0.0",
            "255.255.255.255",
        ] {
            assert!(!is_globally_routable(v4(s)), "{s} must be rejected");
        }
    }

    #[test]
    fn accepts_public_v4() {
        for s in ["1.1.1.1", "8.8.8.8", "93.184.216.34"] {
            assert!(is_globally_routable(v4(s)), "{s} must be accepted");
        }
    }

    #[test]
    fn rejects_non_public_v6() {
        for s in ["::1", "fc00::1", "fd12::1", "fe80::1", "::"] {
            assert!(!is_globally_routable(v6(s)), "{s} must be rejected");
        }
        assert!(!is_globally_routable(v6("::ffff:127.0.0.1")));
        assert!(!is_globally_routable(v6("::ffff:10.0.0.1")));
    }

    #[test]
    fn accepts_public_v6() {
        assert!(is_globally_routable(v6("2606:4700:4700::1111")));
    }
}
