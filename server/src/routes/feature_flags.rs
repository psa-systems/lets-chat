//! LC-1056: three-state runtime rollout for features that are still being
//! proven in production (remote control, transcription).
//!
//! Each [`Flag`] is two rows in the settings.db `settings` KV store, read live
//! with no cache (the same store and posture as `routes::ai_gate`'s LLM flag),
//! so an admin change on `/admin/feature-flags` takes effect on the next
//! request or signal with no restart:
//!
//! - `enabled_key`: `"true"` turns the feature on; absent or anything else is
//!   [`Rollout::Off`].
//! - `audience_key`: `"admins"` narrows an enabled feature to site admins
//!   ([`Rollout::Admins`], for testing live); absent or anything else is
//!   [`Rollout::Everyone`]. Remote control predates the audience row, so an
//!   existing `remote_control_enabled = "true"` deployment keeps reading as
//!   everyone after upgrade.
//!
//! A settings read error resolves to `Off` (the restrictive value) and is
//! logged, mirroring LC-919 / LC-1018.

use crate::db;
use crate::models::User;
use crate::state::AppState;

/// One runtime-gated feature: the settings rows that hold its rollout.
pub struct Flag {
    pub enabled_key: &'static str,
    pub audience_key: &'static str,
}

/// LC-853 remote control. `enabled_key` is the original LC-853 switch row.
pub const REMOTE_CONTROL: Flag = Flag {
    enabled_key: super::ws::REMOTE_CONTROL_ENABLED_KEY,
    audience_key: "remote_control_audience",
};

/// Call transcription and voice-message / clip auto-transcription. Absent
/// rows read as off, so the feature stays dark after upgrade until an admin
/// turns it on.
pub const TRANSCRIPTION: Flag = Flag {
    enabled_key: "transcription_enabled",
    audience_key: "transcription_audience",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rollout {
    Off,
    Admins,
    Everyone,
}

impl Rollout {
    // Admin-tab only; saas compiles no `routes::admin`.
    #[cfg_attr(not(feature = "standalone"), allow(dead_code))]
    /// Parse the admin form value; anything unrecognised is `Off`.
    pub fn from_form(value: Option<&str>) -> Self {
        match value {
            Some("everyone") => Rollout::Everyone,
            Some("admins") => Rollout::Admins,
            _ => Rollout::Off,
        }
    }

    // Admin-tab only; saas compiles no `routes::admin`.
    #[cfg_attr(not(feature = "standalone"), allow(dead_code))]
    pub fn as_str(self) -> &'static str {
        match self {
            Rollout::Off => "off",
            Rollout::Admins => "admins",
            Rollout::Everyone => "everyone",
        }
    }

    pub fn is_on(self) -> bool {
        self != Rollout::Off
    }

    /// May a user with site role `role` use the feature?
    pub fn allows(self, role: &str) -> bool {
        match self {
            Rollout::Off => false,
            Rollout::Admins => role == "admin",
            Rollout::Everyone => true,
        }
    }
}

/// The flag's current rollout, read live from the settings store.
pub async fn rollout(state: &AppState, flag: &Flag) -> Rollout {
    let enabled = db::settings::get_setting(&state.settings, flag.enabled_key).await;
    let audience = db::settings::get_setting(&state.settings, flag.audience_key).await;
    resolve(flag, enabled, audience)
}

fn resolve(
    flag: &Flag,
    enabled: Result<Option<String>, sqlx::Error>,
    audience: Result<Option<String>, sqlx::Error>,
) -> Rollout {
    let (enabled, audience) = match (enabled, audience) {
        (Ok(e), Ok(a)) => (e, a),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(
                error = %e,
                key = flag.enabled_key,
                "feature flag setting read failed; treating flag as off"
            );
            return Rollout::Off;
        }
    };
    if enabled.as_deref() != Some("true") {
        Rollout::Off
    } else if audience.as_deref() == Some("admins") {
        Rollout::Admins
    } else {
        Rollout::Everyone
    }
}

/// May `user` use the feature right now?
pub async fn allows(state: &AppState, flag: &Flag, user: &User) -> bool {
    rollout(state, flag).await.allows(&user.role)
}

/// [`allows`] for a user known only by id (an uploader, a signal's peer). An
/// unknown id or a lookup error is not allowed.
pub async fn allows_user_id(state: &AppState, flag: &Flag, user_id: &str) -> bool {
    let rollout = rollout(state, flag).await;
    match rollout {
        Rollout::Off => false,
        Rollout::Everyone => true,
        Rollout::Admins => matches!(
            db::auth::find_user_by_id(&state.auth, user_id).await,
            Ok(Some(row)) if row.role == "admin"
        ),
    }
}

/// Persist a rollout chosen on the admin tab.
#[cfg_attr(not(feature = "standalone"), allow(dead_code))]
pub async fn set_rollout(
    state: &AppState,
    flag: &Flag,
    rollout: Rollout,
) -> Result<(), sqlx::Error> {
    let enabled = if rollout.is_on() { "true" } else { "false" };
    db::settings::set_setting(&state.settings, flag.enabled_key, enabled).await?;
    let audience = if rollout == Rollout::Admins {
        "admins"
    } else {
        "everyone"
    };
    db::settings::set_setting(&state.settings, flag.audience_key, audience).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: &str) -> Result<Option<String>, sqlx::Error> {
        Ok(Some(v.to_string()))
    }

    #[test]
    fn absent_rows_are_off() {
        assert_eq!(resolve(&TRANSCRIPTION, Ok(None), Ok(None)), Rollout::Off);
    }

    #[test]
    fn enabled_without_audience_is_everyone() {
        // An LC-853 deployment that only ever wrote `remote_control_enabled`.
        assert_eq!(
            resolve(&REMOTE_CONTROL, row("true"), Ok(None)),
            Rollout::Everyone
        );
    }

    #[test]
    fn admins_audience_narrows() {
        assert_eq!(
            resolve(&TRANSCRIPTION, row("true"), row("admins")),
            Rollout::Admins
        );
        assert_eq!(
            resolve(&TRANSCRIPTION, row("false"), row("admins")),
            Rollout::Off
        );
    }

    #[test]
    fn read_error_is_off() {
        assert_eq!(
            resolve(
                &TRANSCRIPTION,
                Err(sqlx::Error::RowNotFound),
                row("everyone")
            ),
            Rollout::Off
        );
        assert_eq!(
            resolve(&TRANSCRIPTION, row("true"), Err(sqlx::Error::RowNotFound)),
            Rollout::Off
        );
    }

    #[test]
    fn allows_by_role() {
        assert!(!Rollout::Off.allows("admin"));
        assert!(Rollout::Admins.allows("admin"));
        assert!(!Rollout::Admins.allows("user"));
        assert!(!Rollout::Admins.allows("moderator"));
        assert!(Rollout::Everyone.allows("user"));
    }

    #[test]
    fn form_values_round_trip() {
        for r in [Rollout::Off, Rollout::Admins, Rollout::Everyone] {
            assert_eq!(Rollout::from_form(Some(r.as_str())), r);
        }
        assert_eq!(Rollout::from_form(None), Rollout::Off);
        assert_eq!(Rollout::from_form(Some("bogus")), Rollout::Off);
    }
}
