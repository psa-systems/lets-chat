//! DEV-300: the fleet-standard `SETUP_DEFAULT_ADMIN` bootstrap, adapted to a
//! pure-RP app.
//!
//! menkent (`api/src/main.rs`), bunyip (`bunyip-api/src/main.rs`) and eform all
//! read one `SETUP_DEFAULT_ADMIN=email:password` variable and seed an admin
//! while no admin exists. lets-chat keeps the same variable so the fleet stays
//! greppable, but has no local password path since the LC-22 cutover: the
//! password half is accepted for format compatibility and never stored.
//!
//! What the seed produces is an UNLINKED, verified-email admin row - exactly
//! the shape `routes::bunyip_sso::resolve_or_provision_user` adopts on first
//! sign-in (the LC-588 path). The developer signs in through the OP and lands
//! on the pre-made admin account instead of racing to be the first user.
//!
//! The seed alone is not sufficient: `mirror_bunyip_admin_role` (LC-413) makes
//! the OP the source of truth for the role on every login, so the OP must also
//! claim `bunyip_role: admin` for that identity or the adopted row is demoted
//! back to `user`. `dev/mock-oidc.py` claims it for its primary identity.
//!
//! Gate: a debug build only, the same dev-detection `routes/dev.rs` uses. A
//! release binary - which is the only thing the fleet ships - never seeds, so
//! the variable cannot mint an admin on a production deployment.

use sqlx::SqlitePool;

use crate::db;

/// What a `SETUP_DEFAULT_ADMIN` value resolves to, before any database work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seed {
    /// Unset, empty, or a release build: seed nothing, say nothing.
    Skip,
    /// Set but unusable. Carries the operator-facing reason.
    Invalid(String),
    /// Seed this admin if none exists.
    Admin { email: String, username: String },
}

/// Pure half of [`ensure_default_admin`], so both sides of the dev gate are
/// testable (a test binary is always a debug build).
pub fn decide(dev_build: bool, raw: Option<&str>) -> Seed {
    let raw = match raw.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => return Seed::Skip,
    };
    // Checked after the presence test so a production deployment that sets the
    // variable by mistake still gets a log line instead of silence.
    if !dev_build {
        return Seed::Invalid(
            "SETUP_DEFAULT_ADMIN is set on a release build; refusing to seed an admin".to_string(),
        );
    }
    let Some((email, _password)) = raw.split_once(':') else {
        return Seed::Invalid("SETUP_DEFAULT_ADMIN must be in format 'email:password'".to_string());
    };
    let email = email.trim();
    if email.is_empty() || !email.contains('@') {
        return Seed::Invalid(format!(
            "SETUP_DEFAULT_ADMIN needs an email address before the ':' (got {email:?})"
        ));
    }
    let local = email.split('@').next().unwrap_or("admin");
    Seed::Admin {
        email: email.to_string(),
        username: crate::models::user::sanitize_handle(local),
    }
}

/// Startup entry point: resolve the gate from this build and this environment,
/// then seed. Idempotent - a second boot finds the admin from the first and
/// skips, so it never creates a duplicate and never errors.
pub async fn ensure_default_admin(pool: &SqlitePool) {
    let raw = std::env::var("SETUP_DEFAULT_ADMIN").ok();
    seed_default_admin(pool, cfg!(debug_assertions), raw.as_deref()).await;
}

/// Both inputs of the gate are arguments, so a test can drive the release-build
/// branch against a real pool and prove it writes nothing.
pub async fn seed_default_admin(pool: &SqlitePool, dev_build: bool, raw: Option<&str>) {
    match decide(dev_build, raw) {
        Seed::Skip => {}
        Seed::Invalid(why) => {
            tracing::error!(target: "setup_admin", "{why}");
        }
        Seed::Admin { email, username } => {
            let username = match pick_handle(pool, &username).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    tracing::error!(
                        target: "setup_admin",
                        "no free handle near {username:?}; skipping SETUP_DEFAULT_ADMIN"
                    );
                    return;
                }
                Err(e) => {
                    tracing::error!(target: "setup_admin", error = %e, "handle lookup failed; skipping SETUP_DEFAULT_ADMIN");
                    return;
                }
            };
            match db::auth::create_default_admin(pool, &username, &email).await {
                Ok(Some(id)) => tracing::warn!(
                    target: "setup_admin",
                    user_id = %id, %username, %email,
                    "DEV ONLY: seeded the default admin from SETUP_DEFAULT_ADMIN; it is claimed by the first sign-in with this verified email"
                ),
                Ok(None) => tracing::info!(
                    target: "setup_admin",
                    "admin user(s) already exist, skipping SETUP_DEFAULT_ADMIN"
                ),
                Err(e) => tracing::error!(
                    target: "setup_admin", error = %e,
                    "failed to seed the default admin from SETUP_DEFAULT_ADMIN"
                ),
            }
        }
    }
}

/// First free handle at or near `base`, mirroring `pick_username` in
/// `routes/bunyip_sso.rs`. `None` when every candidate is taken.
async fn pick_handle(pool: &SqlitePool, base: &str) -> Result<Option<String>, sqlx::Error> {
    if !db::auth::username_exists(pool, base).await? {
        return Ok(Some(base.to_string()));
    }
    for n in 2..=5u32 {
        let candidate = format!("{base}-{n}");
        if !db::auth::username_exists(pool, &candidate).await? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{decide, Seed};

    #[test]
    fn a_release_build_never_seeds() {
        // The gate that keeps a production deployment safe: same value, same
        // everything, only the build profile differs.
        let raw = Some("admin@a8n.run:admin1234");
        assert!(matches!(decide(false, raw), Seed::Invalid(_)));
        assert!(matches!(decide(true, raw), Seed::Admin { .. }));
    }

    #[test]
    fn an_unset_or_blank_value_is_silent_on_both_sides_of_the_gate() {
        for raw in [None, Some(""), Some("   ")] {
            assert_eq!(decide(true, raw), Seed::Skip, "{raw:?}");
            assert_eq!(decide(false, raw), Seed::Skip, "{raw:?}");
        }
    }

    #[test]
    fn the_email_half_becomes_the_email_and_the_handle() {
        assert_eq!(
            decide(true, Some(" Dev.User@example.test : hunter2 ")),
            Seed::Admin {
                email: "Dev.User@example.test".to_string(),
                username: "Dev.User".to_string(),
            }
        );
    }

    #[test]
    fn a_malformed_value_is_reported_not_guessed() {
        for raw in ["admin@a8n.run", ":pw", "admin:pw", "  :  "] {
            assert!(
                matches!(decide(true, Some(raw)), Seed::Invalid(_)),
                "{raw:?}"
            );
        }
    }
}
