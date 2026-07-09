use serde::{Deserialize, Serialize};

/// Full user record. Only used server-side - never sent to the client.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub password_hash: String,
    pub role: String,
    pub is_banned: bool,
    pub ban_reason: Option<String>,
    pub banned_until: Option<String>,
    pub is_muted: bool,
    pub muted_until: Option<String>,
    pub mute_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub read_receipts_enabled: bool,
    pub bio: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub last_active_at: String,
    pub is_profile_public: bool,
    pub notify_browser_enabled: bool,
    pub notify_sound_enabled: bool,
    pub notify_push_enabled: bool,
    pub notify_email_digest_enabled: bool,
    pub notify_login_alerts_enabled: bool,
    /// LC-77-REPLY: per-user opt-in for per-message mention + DM email
    /// notifications. Distinct from the hourly digest opt-in. OFF by default;
    /// mention emails are noisy and operators opt their users in deliberately.
    pub notify_email_activity_enabled: bool,
    pub last_ws_seen_at: Option<String>,
    pub last_digest_sent_at: Option<String>,
    /// LC-88: recurring Do Not Disturb schedule (quiet hours), JSON or NULL.
    /// See `crate::dnd::Schedule` for the shape.
    pub dnd_schedule_json: Option<String>,
    /// LC-88: explicit manual pause instant (ISO-8601 UTC) or NULL. When in
    /// the future it supersedes the schedule.
    pub dnd_paused_until: Option<String>,
    /// Optional notification address. Used by the email digest tick and
    /// eventually by other email features. Deliberately NOT mirrored
    /// onto the public `User` projection - email is recipient metadata,
    /// not identity, and should not flow into handler/template contexts.
    pub email: Option<String>,
    pub totp_secret_encrypted: Option<Vec<u8>>,
    pub totp_nonce: Option<Vec<u8>>,
    pub totp_enabled: bool,
    pub totp_recovery_hashes: Option<String>,
    /// LC-73: true for machine (bot) identities. Bots authenticate only via
    /// API tokens; the cookie login path rejects them.
    pub is_bot: bool,
    /// LC-100: preferred UI locale code (e.g. "en", "es"), or NULL to fall
    /// back to the request's Accept-Language.
    pub locale: Option<String>,
    /// LC-541: preferred UI mode ("light"/"dark"/"hc-light"/"hc-dark"/"system"),
    /// or NULL = system. (Renamed from `theme` when the palette axis was added.)
    pub theme_mode: Option<String>,
    /// LC-541: preferred palette ("blue-harbor"/"cobalt"/"ink-ice"/"arctic"/
    /// "deep-sea"/"royal-navy"), or NULL = blue-harbor.
    pub theme_palette: Option<String>,
    /// LC-194: preferred UI density ("comfortable"/"compact"), NULL = comfortable.
    pub density: Option<String>,
    /// LC-533: short pronoun string (e.g. "she/her"), or NULL. Capped on write.
    pub pronouns: Option<String>,
    /// LC-533: newline-separated http(s) profile links, validated on write.
    pub profile_links: Option<String>,
    /// LC-533: IANA timezone name for the profile "local time" line, or NULL.
    pub timezone: Option<String>,
}

/// Public user info safe to send to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub is_muted: bool,
    pub muted_until: Option<String>,
    pub is_banned: bool,
    pub ban_reason: Option<String>,
    pub banned_until: Option<String>,
    pub created_at: String,
    pub read_receipts_enabled: bool,
    pub bio: Option<String>,
    pub avatar_ext: Option<String>,
    pub status: String,
    pub custom_status: Option<String>,
    pub last_active_at: String,
    pub is_profile_public: bool,
    pub notify_browser_enabled: bool,
    pub notify_sound_enabled: bool,
    pub notify_push_enabled: bool,
    pub notify_email_digest_enabled: bool,
    pub notify_login_alerts_enabled: bool,
    /// LC-77-REPLY: per-message mention + DM email opt-in. See UserRecord.
    pub notify_email_activity_enabled: bool,
    pub totp_enabled: bool,
    /// LC-73: true for bot identities. Drives the "bot" badge in chat.
    pub is_bot: bool,
    /// LC-100: preferred UI locale code, or None for Accept-Language fallback.
    pub locale: Option<String>,
    /// LC-541: preferred UI mode ("light"/"dark"/"hc-light"/"hc-dark"/"system"),
    /// or None = system. (Renamed from `theme` when the palette axis was added.)
    pub theme_mode: Option<String>,
    /// LC-541: preferred palette ("blue-harbor"/"cobalt"/"ink-ice"/"arctic"/
    /// "deep-sea"/"royal-navy"), or None = blue-harbor.
    pub theme_palette: Option<String>,
    /// LC-194: preferred UI density ("comfortable"/"compact"), None = comfortable.
    pub density: Option<String>,
    /// LC-533: short pronoun string (e.g. "she/her"), or None.
    pub pronouns: Option<String>,
    /// LC-533: newline-separated http(s) profile links (validated on write).
    pub profile_links: Option<String>,
    /// LC-533: IANA timezone name for the "local time" line, or None.
    pub timezone: Option<String>,
    /// LC-88: true when Do Not Disturb is currently active (manual pause or a
    /// schedule window). Computed at projection time from the record's DND
    /// columns against the wall clock; surfaces the "do not disturb" badge.
    pub dnd_active: bool,
}

impl User {
    /// LC-541: saved mode preference, defaulting to "system" when unset.
    pub fn theme_mode_or_system(&self) -> &str {
        match self.theme_mode.as_deref() {
            Some(t @ ("light" | "dark" | "hc-light" | "hc-dark" | "system")) => t,
            _ => "system",
        }
    }

    /// LC-541: saved palette preference, defaulting to "blue-harbor" when unset.
    pub fn theme_palette_or_default(&self) -> &str {
        match self.theme_palette.as_deref() {
            Some(
                p @ ("blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy"),
            ) => p,
            _ => "blue-harbor",
        }
    }

    /// LC-194: saved density preference, defaulting to "comfortable" when unset.
    pub fn density_or_default(&self) -> &str {
        match self.density.as_deref() {
            Some(d @ ("comfortable" | "compact")) => d,
            _ => "comfortable",
        }
    }

    /// Trimmed display_name when set and non-empty, otherwise the username.
    pub fn display_label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }

    /// LC-535: whether the global posting mute is currently in force. A NULL
    /// `muted_until` is a permanent mute; a non-NULL one auto-expires once the
    /// wall clock passes it. The background sweep (`clear_expired_mutes`) nulls
    /// the row shortly after, but every posting gate checks here so a user is
    /// never blocked past their own expiry. An unparseable timestamp fails safe
    /// to "still muted" rather than silently lifting the mute.
    pub fn mute_in_effect(&self) -> bool {
        self.is_muted && mute_active(self.muted_until.as_deref(), chrono::Utc::now())
    }

    /// Presence status to render in the avatar badge. LC-88: an active Do Not
    /// Disturb shows the "do not disturb" (red) dot regardless of the stored
    /// presence status, so others can see the user is quiet.
    pub fn effective_status(&self) -> &str {
        if self.dnd_active {
            "dnd"
        } else {
            &self.status
        }
    }
}

/// LC-535: is a mute with the given `muted_until` still active at `now`?
/// `None` is a permanent mute (always active). A non-NULL value is the UTC
/// `YYYY-MM-DD HH:MM:SS` string written by `mute_user`; once `now` reaches it
/// the mute has lifted. An unparseable string fails safe to active so a
/// corrupt timestamp can never silently unmute someone.
fn mute_active(muted_until: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    match muted_until {
        None => true,
        Some(s) => match chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            Ok(dt) => now < dt.and_utc(),
            Err(_) => true,
        },
    }
}

/// LC-533: write-path caps for the profile extras. Enforced by the settings
/// handler; the hovercard trusts the stored value.
pub const MAX_PRONOUNS_CHARS: usize = 40;
pub const MAX_PROFILE_LINKS: usize = 5;
pub const MAX_LINK_CHARS: usize = 200;

/// LC-533: validate and normalise the free-text links box. Input is one URL
/// per line; blank lines are dropped. Each surviving line must be an http(s)
/// URL within the length cap, and there is a hard cap on the count. Returns the
/// normalised newline-joined string (`None` when empty) or a human-readable
/// error for `settings_error`. Only the scheme + length are checked here; the
/// stored value is rendered as an href, so the scheme allowlist is what keeps a
/// `javascript:`/`data:` URL out of the anchor.
pub fn validate_profile_links(input: &str) -> Result<Option<String>, String> {
    let mut links: Vec<&str> = Vec::new();
    for line in input.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > MAX_LINK_CHARS {
            return Err(format!(
                "A profile link exceeds {MAX_LINK_CHARS} characters."
            ));
        }
        if !(t.starts_with("http://") || t.starts_with("https://")) {
            return Err("Profile links must start with http:// or https://.".to_string());
        }
        links.push(t);
    }
    if links.len() > MAX_PROFILE_LINKS {
        return Err(format!(
            "At most {MAX_PROFILE_LINKS} profile links are allowed."
        ));
    }
    if links.is_empty() {
        Ok(None)
    } else {
        Ok(Some(links.join("\n")))
    }
}

/// LC-533: split a stored links blob into individual URLs for rendering.
/// Defensive: re-drops any blank/non-http(s) line so a value written before a
/// future validation change can never render a bad href.
pub fn profile_link_list(raw: &str) -> Vec<&str> {
    raw.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
        .collect()
}

/// LC-533: current wall-clock time in the user's IANA timezone, formatted for
/// the hovercard (e.g. "3:45 PM EDT"). `None` when the tz is empty or unknown,
/// so an unset or stale timezone simply hides the line.
pub fn local_time_in(tz_name: &str, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let tz: chrono_tz::Tz = tz_name.parse().ok()?;
    Some(now.with_timezone(&tz).format("%-I:%M %p %Z").to_string())
}

impl From<UserRecord> for User {
    fn from(r: UserRecord) -> Self {
        // Compute DND state against the current instant before the record is
        // consumed below. `is_suppressed` early-outs cheaply when neither a
        // pause nor a schedule is set, which is the common case.
        let dnd_active = crate::dnd::is_suppressed(&r, chrono::Utc::now());
        User {
            id: r.id,
            username: r.username,
            display_name: r.display_name,
            role: r.role,
            is_muted: r.is_muted,
            muted_until: r.muted_until,
            is_banned: r.is_banned,
            ban_reason: r.ban_reason,
            banned_until: r.banned_until,
            created_at: r.created_at,
            read_receipts_enabled: r.read_receipts_enabled,
            bio: r.bio,
            avatar_ext: r.avatar_ext,
            status: r.status,
            custom_status: r.custom_status,
            last_active_at: r.last_active_at,
            is_profile_public: r.is_profile_public,
            notify_browser_enabled: r.notify_browser_enabled,
            notify_sound_enabled: r.notify_sound_enabled,
            notify_push_enabled: r.notify_push_enabled,
            notify_email_digest_enabled: r.notify_email_digest_enabled,
            notify_login_alerts_enabled: r.notify_login_alerts_enabled,
            notify_email_activity_enabled: r.notify_email_activity_enabled,
            totp_enabled: r.totp_enabled,
            is_bot: r.is_bot,
            locale: r.locale,
            theme_mode: r.theme_mode,
            theme_palette: r.theme_palette,
            density: r.density,
            pronouns: r.pronouns,
            profile_links: r.profile_links,
            timezone: r.timezone,
            dnd_active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mute_active;
    use chrono::{TimeZone, Utc};

    #[test]
    fn permanent_mute_never_expires() {
        let now = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
        assert!(mute_active(None, now));
    }

    #[test]
    fn timed_mute_active_before_and_lifted_after() {
        let until = "2026-07-04 12:00:00";
        let before = Utc.with_ymd_and_hms(2026, 7, 4, 11, 59, 59).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 1).unwrap();
        assert!(
            mute_active(Some(until), before),
            "still muted before expiry"
        );
        assert!(!mute_active(Some(until), after), "lifted after expiry");
    }

    #[test]
    fn unparseable_timestamp_stays_muted() {
        let now = Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap();
        assert!(mute_active(Some("not-a-date"), now));
    }

    // LC-533 profile extras.
    use super::{local_time_in, profile_link_list, validate_profile_links, MAX_PROFILE_LINKS};

    #[test]
    fn links_validated_normalised_and_capped() {
        // Blank lines dropped, order preserved, whitespace trimmed.
        assert_eq!(
            validate_profile_links("  https://a.example  \n\nhttp://b.example\n"),
            Ok(Some("https://a.example\nhttp://b.example".to_string()))
        );
        // Empty input clears the field.
        assert_eq!(validate_profile_links("   \n  "), Ok(None));
        // Non-http(s) scheme rejected (keeps javascript:/data: out of hrefs).
        assert!(validate_profile_links("javascript:alert(1)").is_err());
        assert!(validate_profile_links("ftp://x.example").is_err());
        // Over the count cap.
        let many = (0..MAX_PROFILE_LINKS + 1)
            .map(|i| format!("https://s{i}.example"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(validate_profile_links(&many).is_err());
    }

    #[test]
    fn link_list_defensively_drops_bad_lines() {
        let stored = "https://ok.example\njavascript:bad\n\nhttp://ok2.example";
        assert_eq!(
            profile_link_list(stored),
            vec!["https://ok.example", "http://ok2.example"]
        );
    }

    #[test]
    fn local_time_formats_known_tz_and_hides_unknown() {
        // 2026-07-04 16:00:00 UTC is 12:00 PM in New York (EDT, UTC-4).
        let now = Utc.with_ymd_and_hms(2026, 7, 4, 16, 0, 0).unwrap();
        assert_eq!(
            local_time_in("America/New_York", now),
            Some("12:00 PM EDT".to_string())
        );
        assert_eq!(local_time_in("", now), None);
        assert_eq!(local_time_in("Not/AZone", now), None);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::User;

    fn user_with(mode: Option<&str>, palette: Option<&str>) -> User {
        User {
            id: "u1".to_string(),
            username: "u1".to_string(),
            display_name: None,
            role: "user".to_string(),
            is_muted: false,
            muted_until: None,
            is_banned: false,
            ban_reason: None,
            banned_until: None,
            created_at: String::new(),
            read_receipts_enabled: true,
            bio: None,
            avatar_ext: None,
            status: "active".to_string(),
            custom_status: None,
            last_active_at: String::new(),
            is_profile_public: false,
            notify_browser_enabled: false,
            notify_sound_enabled: false,
            notify_push_enabled: false,
            notify_email_digest_enabled: false,
            notify_login_alerts_enabled: false,
            notify_email_activity_enabled: false,
            totp_enabled: false,
            is_bot: false,
            locale: None,
            theme_mode: mode.map(str::to_string),
            theme_palette: palette.map(str::to_string),
            density: None,
            pronouns: None,
            profile_links: None,
            timezone: None,
            dnd_active: false,
        }
    }

    #[test]
    fn mode_defaults_to_system() {
        assert_eq!(user_with(None, None).theme_mode_or_system(), "system");
        assert_eq!(user_with(Some("dark"), None).theme_mode_or_system(), "dark");
        assert_eq!(
            user_with(Some("bogus"), None).theme_mode_or_system(),
            "system"
        );
    }

    #[test]
    fn palette_defaults_to_blue_harbor() {
        assert_eq!(
            user_with(None, None).theme_palette_or_default(),
            "blue-harbor"
        );
        assert_eq!(
            user_with(None, Some("cobalt")).theme_palette_or_default(),
            "cobalt"
        );
        assert_eq!(
            user_with(None, Some("bogus")).theme_palette_or_default(),
            "blue-harbor"
        );
    }
}
