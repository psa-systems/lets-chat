//! IP -> country resolution via an IP2Location LITE database (LC-580).
//!
//! Reads the same IP2Location LITE `.BIN` DB the ecosystem already deploys
//! (`IP2LOCATION_DB_PATH`, e.g. `/data/IP2LOCATION-LITE-DB11.BIN`). It is used
//! only to detect a country-level change between a user's logins; when no DB is
//! configured the login-location-alert feature is disabled (the resolver is
//! never constructed). Lookups are offline: no per-login external call, and no
//! client IP is sent to a third party.

use std::net::IpAddr;

use ip2location::{Record, DB};

/// An opened IP2Location database, queried for the country of a client IP.
pub struct GeoipResolver {
    db: DB,
}

/// A resolved country: the ISO 3166-1 alpha-2 `code` (stable, used for the
/// change comparison + storage) and a human-readable `name` (shown in the
/// alert email, e.g. "United States" rather than "US").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCountry {
    pub code: String,
    pub name: String,
}

impl GeoipResolver {
    /// Build the resolver from the environment, mirroring the mailer / stt /
    /// llm `from_env` pattern. Returns `None` (feature disabled) when
    /// `IP2LOCATION_DB_PATH` is unset/empty or the `.BIN` fails to load; the
    /// server still boots normally.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("IP2LOCATION_DB_PATH").ok()?;
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        match Self::open(path) {
            Ok(resolver) => {
                tracing::info!(target: "geoip", path, "IP2Location resolver loaded (LC-580)");
                Some(resolver)
            }
            Err(e) => {
                tracing::warn!(
                    target: "geoip",
                    path,
                    error = %e,
                    "failed to load IP2Location DB; login-location alerts disabled",
                );
                None
            }
        }
    }

    /// Open the IP2Location `.BIN` database at `path`.
    pub fn open(path: &str) -> Result<Self, String> {
        let db = DB::from_file(path).map_err(|e| format!("IP2Location DB load failed: {e}"))?;
        Ok(Self { db })
    }

    /// Resolve `ip` to its country: the ISO 3166-1 alpha-2 `code` (stable, used
    /// for the change comparison + storage) and the human-readable `name` (for
    /// display in the alert email). `None` when the IP is not resolvable
    /// (private / reserved / unknown ranges carry no country).
    pub fn resolve(&self, ip: IpAddr) -> Option<ResolvedCountry> {
        let country = match self.db.ip_lookup(ip).ok()? {
            Record::LocationDb(rec) => rec.country,
            Record::ProxyDb(_) => None,
        }?;
        let code = normalize_country_code(&country.short_name)?;
        // Fall back to the code when the DB carries no usable long name.
        let name = normalize_country_code(&country.long_name).unwrap_or_else(|| code.clone());
        Some(ResolvedCountry { code, name })
    }
}

/// Normalize an IP2Location country field into a usable ISO 3166-1 alpha-2
/// code, or `None`. IP2Location stores `"-"` (and occasionally a blank) for
/// unknown / reserved ranges; those are not real countries and must never be
/// treated as a login-location change (LC-580).
fn normalize_country_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    if code.is_empty() || code == "-" {
        None
    } else {
        Some(code.to_string())
    }
}

/// True only for genuinely public client IPs. A request that arrives without a
/// real client IP (loopback / RFC1918 / unique-local / link-local /
/// unspecified / broadcast) must never drive a country-change alert (LC-580).
pub fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local())
        }
    }
}

/// The action to take for a resolved login country, given the country recorded
/// at the user's previous login. Pure so the branch logic is unit-testable in
/// isolation.
#[derive(Debug, PartialEq, Eq)]
pub enum LocationDecision {
    /// No prior country on record: store this one silently, no alert.
    Record,
    /// Same country as last time: do nothing.
    Unchanged,
    /// Country differs from last time: alert the user, then store the new one.
    Alert,
}

/// Decide what to do for the `current` login country given the `previous` one.
pub fn location_decision(previous: Option<&str>, current: &str) -> LocationDecision {
    match previous {
        None => LocationDecision::Record,
        Some(prev) if prev == current => LocationDecision::Unchanged,
        Some(_) => LocationDecision::Alert,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_placeholder_and_blank() {
        assert_eq!(normalize_country_code("-"), None);
        assert_eq!(normalize_country_code(""), None);
        assert_eq!(normalize_country_code("   "), None);
    }

    #[test]
    fn normalize_keeps_and_trims_iso2() {
        assert_eq!(normalize_country_code("US"), Some("US".to_string()));
        assert_eq!(normalize_country_code(" GB "), Some("GB".to_string()));
    }

    #[test]
    fn public_ip_filter() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.10.10",
            "0.0.0.0",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(
                !is_public_ip(&ip.parse().unwrap()),
                "{ip} should be treated as non-public"
            );
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(
                is_public_ip(&ip.parse().unwrap()),
                "{ip} should be treated as public"
            );
        }
    }

    #[test]
    fn decision_branches() {
        assert_eq!(location_decision(None, "US"), LocationDecision::Record);
        assert_eq!(
            location_decision(Some("US"), "US"),
            LocationDecision::Unchanged
        );
        assert_eq!(location_decision(Some("US"), "GB"), LocationDecision::Alert);
    }
}
