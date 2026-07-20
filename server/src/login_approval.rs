//! LC-587: suspicious-login notify-and-approve gate.
//!
//! The Lets-Chat analog of mokosh PMS-658, adapted to the pure-RP Bunyip SSO
//! callback (there is no local password re-POST to hang a second factor off).
//! When the opt-in `LOGIN_APPROVAL_ENABLED` flag is set, a login that has
//! already authenticated at the callback but looks suspicious - a new country
//! (reusing the LC-580 geoip signal) and/or a new device - does NOT mint a
//! session. Instead a single-use 6-digit code is emailed and an interstitial
//! page asks the user to approve the sign-in by entering it. This notifies
//! rather than locks (an account lock frustrates users; PMS-658 decision).
//!
//! Fail-open everywhere: if the code cannot be issued or delivered (no verified
//! email, no mailer, DB error) the login proceeds rather than locking the user
//! out of their own account. A suspicious context is only recorded as the new
//! baseline once it has been cleared (not flagged) or explicitly approved.
//!
//! Device signal (SSR adaptation): the "device id" is a first-party `device_id`
//! cookie Lets-Chat sets on a cleared login; only its SHA-256 hash is stored. A
//! login that presents a `device_id` hash absent from the user's known set is a
//! new-device signal, but only once the user already has >= 1 known device (the
//! first device is baseline). A login that presents no `device_id` contributes
//! no device signal, so the gate degrades to country-only (fail-open). This is
//! weaker than PMS-658's SPA-supplied persistent id; the country signal is the
//! primary one for Lets-Chat.

use askama::Template;
use sqlx::SqlitePool;

use crate::db;
use crate::geoip::LocationDecision;
use crate::state::AppState;
use crate::views::login_approval::{LoginApprovalHtml, LoginApprovalText};

/// Wrong-code attempts allowed against one challenge before it is burned. Keeps
/// the 6-digit space (1e6) infeasible to brute-force: 5 tries per emailed code.
pub const MAX_ATTEMPTS: i64 = 5;

/// Outcome of assessing a just-authenticated login (gate enabled only).
pub enum LoginGate {
    /// Not suspicious: mint the session, then record the resolved country /
    /// device as the new baseline. `device_hash` is `Some` only when the client
    /// presented a `device_id`.
    Clear {
        country: Option<String>,
        device_hash: Option<String>,
    },
    /// Suspicious: a challenge row was inserted and a code emailed. Render the
    /// interstitial for `token` and withhold the session.
    Challenge { token: String },
}

/// Outcome of verifying a submitted approval code.
pub enum VerifyOutcome {
    /// Correct code: mint the session for `user_id`, then apply the flagged
    /// context as the new baseline.
    Approved {
        user_id: String,
        country: Option<String>,
        device_hash: Option<String>,
    },
    /// Wrong code, attempts remain: re-render the interstitial with an error.
    Wrong,
    /// Unknown / expired / consumed / attempt-cap-exhausted: restart login.
    Invalid,
}

/// SHA-256 hex of a `device_id` cookie value; only the hash is ever stored.
pub fn hash_device(device_id: &str) -> String {
    sha256_hex(device_id)
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a zero-padded 6-digit approval code from the OS CSPRNG.
fn gen_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32))
}

/// Pure new-device rule: a presented device is "new" only when the user already
/// has known devices and this one is not among them. Unit-tested in isolation.
fn device_is_new(has_known: bool, is_known: bool) -> bool {
    has_known && !is_known
}

/// Pure combine rule: a login is suspicious on either signal.
fn is_suspicious(new_country: bool, new_device: bool) -> bool {
    new_country || new_device
}

/// Assess a just-authenticated login for suspicious signals. Called only when
/// the gate is enabled. On `Clear` the caller mints the session then calls
/// [`apply_baseline`]; on `Challenge` the caller renders the interstitial and
/// withholds the session.
pub async fn guard(
    state: &AppState,
    user_id: &str,
    ip: Option<&str>,
    ua: Option<&str>,
    device_id: Option<&str>,
) -> LoginGate {
    let country = resolve_country(state, ip);
    let device_hash = device_id.map(hash_device);

    // New-country signal: a change against the recorded last login country
    // (reuses the exact LC-580 decision so the two features stay consistent).
    let new_country = match &country {
        Some(c) => {
            let last = db::auth::get_last_login_country(&state.auth, user_id)
                .await
                .unwrap_or(None);
            matches!(
                crate::geoip::location_decision(last.as_deref(), &c.code),
                LocationDecision::Alert
            )
        }
        None => false,
    };

    // New-device signal: known-device set non-empty AND this hash absent.
    let new_device = match &device_hash {
        Some(h) => {
            let has_known = db::auth::has_known_device(&state.auth, user_id)
                .await
                .unwrap_or(false);
            // On a lookup error default to "known" so a DB blip never over-gates.
            let known = db::auth::is_known_device(&state.auth, user_id, h)
                .await
                .unwrap_or(true);
            device_is_new(has_known, known)
        }
        None => false,
    };

    let country_code = country.as_ref().map(|c| c.code.clone());
    if !is_suspicious(new_country, new_device) {
        return LoginGate::Clear {
            country: country_code,
            device_hash,
        };
    }

    // Suspicious: try to issue a challenge. If we cannot deliver a code (no
    // verified email / no mailer / DB error) fail OPEN and let the login
    // through, WITHOUT blessing the suspicious context as the new baseline, so
    // it is re-evaluated next time.
    match issue_challenge(
        state,
        user_id,
        country.as_ref(),
        device_hash.as_deref(),
        ip,
        ua,
    )
    .await
    {
        Some(token) => LoginGate::Challenge { token },
        None => LoginGate::Clear {
            country: None,
            device_hash: None,
        },
    }
}

/// Insert a pending challenge and email the code. Returns the challenge token,
/// or `None` when the code cannot be delivered (caller then fails open). Every
/// failure is logged and swallowed.
async fn issue_challenge(
    state: &AppState,
    user_id: &str,
    country: Option<&crate::geoip::ResolvedCountry>,
    device_hash: Option<&str>,
    ip: Option<&str>,
    ua: Option<&str>,
) -> Option<String> {
    // Deliverability gates (a security control, so the per-user login-alert
    // opt-out does NOT apply here - only whether we can actually reach them).
    let user = match db::auth::find_user_by_id(&state.auth, user_id).await {
        Ok(Some(u)) => u,
        _ => return None,
    };
    let email = user.email.as_deref().filter(|s| !s.is_empty())?;
    let verified = matches!(
        db::auth::get_user_email_verified_at(&state.auth, user_id).await,
        Ok(Some(_))
    );
    if !verified {
        return None;
    }
    let mailer = state.mailer.as_ref()?;

    let code = gen_code();
    let code_hash = sha256_hex(&code);
    let token = crate::oidc::new_random_token();
    let country_code = country.map(|c| c.code.as_str());
    if let Err(e) = db::auth::insert_login_approval(
        &state.auth,
        &token,
        user_id,
        &code_hash,
        country_code,
        device_hash,
        ip,
        ua,
    )
    .await
    {
        tracing::error!(target: "login_approval", user_id, error = %e, "insert_login_approval failed");
        return None;
    }

    let country_name = country
        .map(|c| c.name.as_str())
        .unwrap_or("an unrecognized location");
    let ip_str = ip.unwrap_or("unknown");
    let when = chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string();
    let device_label = ua.unwrap_or("unknown");
    let base = state.base_url.trim_end_matches('/');
    let settings_url = format!("{base}/settings");

    let text = LoginApprovalText {
        username: &user.username,
        code: &code,
        country: country_name,
        device_label,
        ip: ip_str,
        when: &when,
        settings_url: &settings_url,
    };
    let html = LoginApprovalHtml {
        username: &user.username,
        code: &code,
        country: country_name,
        device_label,
        ip: ip_str,
        when: &when,
        settings_url: &settings_url,
    };
    let (Ok(text_body), Ok(html_body)) = (text.render(), html.render()) else {
        tracing::warn!(target: "login_approval", user_id, "login_approval template render failed");
        return None;
    };
    let subject = "Approve your sign-in to Let's Chat";
    if let Err(e) = mailer
        .send_multipart(email, subject, &text_body, &html_body)
        .await
    {
        // The row is inserted but the code never arrived: the user cannot
        // approve, so this must fail open. Consume the orphan row and proceed.
        tracing::warn!(target: "login_approval", user_id, error = %e, "approval email failed; failing open");
        let _ = db::auth::consume_login_approval(&state.auth, &token).await;
        return None;
    }
    tracing::info!(target: "login_approval", user_id, "suspicious login challenged");
    Some(token)
}

/// Verify a submitted approval code against its challenge. Takes the auth pool
/// directly (not `AppState`) so the flagged -> approved recovery is testable
/// without standing up the full application state.
pub async fn verify(pool: &SqlitePool, token: &str, code: &str) -> VerifyOutcome {
    // Claim a guess BEFORE comparing. The claim is a single conditional UPDATE
    // that both validates the challenge and spends an attempt, so at most
    // MAX_ATTEMPTS codes are ever compared against one challenge. Reading the
    // count first and bumping afterwards bounded nothing under concurrency:
    // simultaneous submissions all read the same pre-bump value and every one
    // of them got a comparison.
    let approval = match db::auth::claim_login_approval_attempt(pool, token, MAX_ATTEMPTS).await {
        Ok(Some(a)) => a,
        // Unknown, expired, already consumed, or out of attempts.
        Ok(None) => return VerifyOutcome::Invalid,
        Err(e) => {
            tracing::error!(target: "login_approval", error = %e, "claim_login_approval_attempt failed");
            return VerifyOutcome::Invalid;
        }
    };
    if sha256_hex(code) == approval.code_hash {
        // Single-use: the conditional consume is the atomic winner-take-all
        // gate (LC-601). Mint a session only when THIS call consumed the row;
        // if a concurrent request already consumed it we lost the race and must
        // not mint a second session.
        return match db::auth::consume_login_approval(pool, token).await {
            Ok(1) => VerifyOutcome::Approved {
                user_id: approval.user_id,
                country: approval.country,
                device_hash: approval.device_hash,
            },
            _ => VerifyOutcome::Invalid,
        };
    }
    // Wrong code. The attempt is already spent by the claim; burn the challenge
    // outright when this caller took the last slot, so a spent challenge cannot
    // sit around unconsumed.
    if approval.attempts >= MAX_ATTEMPTS {
        let _ = db::auth::consume_login_approval(pool, token).await;
        return VerifyOutcome::Invalid;
    }
    VerifyOutcome::Wrong
}

/// Record a cleared/approved login's country + device as the new baseline, so a
/// repeat login from the same context is not flagged again. Best-effort.
pub async fn apply_baseline(
    pool: &SqlitePool,
    user_id: &str,
    country: Option<&str>,
    device_hash: Option<&str>,
    user_agent: Option<&str>,
) {
    if let Some(c) = country {
        if let Err(e) = db::auth::set_last_login_country(pool, user_id, c).await {
            tracing::warn!(target: "login_approval", user_id, error = %e, "set_last_login_country failed");
        }
    }
    if let Some(h) = device_hash {
        if let Err(e) = db::auth::record_known_device(pool, user_id, h, user_agent).await {
            tracing::warn!(target: "login_approval", user_id, error = %e, "record_known_device failed");
        }
    }
}

/// Resolve a client IP string to a country, applying the same public-IP filter
/// and geoip kill-switch as LC-580. `None` when geoip is disabled or the IP is
/// missing / private / unresolvable.
fn resolve_country(state: &AppState, ip: Option<&str>) -> Option<crate::geoip::ResolvedCountry> {
    let geoip = state.geoip.as_ref()?;
    let parsed = ip.and_then(|s| s.parse::<std::net::IpAddr>().ok())?;
    if !crate::geoip::is_public_ip(&parsed) {
        return None;
    }
    geoip.resolve(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_code_is_six_digits() {
        for _ in 0..200 {
            let c = gen_code();
            assert_eq!(c.len(), 6, "code {c} not 6 chars");
            assert!(
                c.chars().all(|ch| ch.is_ascii_digit()),
                "code {c} non-digit"
            );
        }
    }

    #[test]
    fn sha256_hex_is_stable_and_hex() {
        let h = sha256_hex("123456");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, sha256_hex("123456"));
        assert_ne!(h, sha256_hex("123457"));
    }

    #[test]
    fn new_device_only_when_user_has_known_devices() {
        // First-ever device (no known devices yet): baseline, not new.
        assert!(!device_is_new(false, false));
        // A known device is never new.
        assert!(!device_is_new(true, true));
        // Unknown device once a baseline exists: new.
        assert!(device_is_new(true, false));
    }

    #[test]
    fn suspicious_on_either_signal() {
        assert!(!is_suspicious(false, false));
        assert!(is_suspicious(true, false));
        assert!(is_suspicious(false, true));
        assert!(is_suspicious(true, true));
    }
}
