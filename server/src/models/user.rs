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
    /// LC-88: true when Do Not Disturb is currently active (manual pause or a
    /// schedule window). Computed at projection time from the record's DND
    /// columns against the wall clock; surfaces the "do not disturb" badge.
    pub dnd_active: bool,
}

impl User {
    /// Trimmed display_name when set and non-empty, otherwise the username.
    pub fn display_label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
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
            dnd_active,
        }
    }
}
