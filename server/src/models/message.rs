use serde::{Deserialize, Serialize};

/// LC-547: allowed self-destruct TTLs. Maps a composer form token to the
/// absolute expiry stamp (`"%Y-%m-%d %H:%M:%S"` UTC, the same shape as
/// `datetime('now')`, so the sweep's `expires_at <= datetime('now')` compares
/// lexicographically). Any token outside this closed set - including `""`,
/// `"off"`, or a client-forged value - returns `None`, meaning the message is
/// permanent. Keeping the mapping server-side means the client cannot request
/// an arbitrary TTL.
pub fn ephemeral_expires_at(ttl: &str, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let dur = match ttl {
        "5m" => chrono::Duration::minutes(5),
        "1h" => chrono::Duration::hours(1),
        "1d" => chrono::Duration::days(1),
        "7d" => chrono::Duration::days(7),
        _ => return None,
    };
    Some((now + dur).format("%Y-%m-%d %H:%M:%S").to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    /// `Some(N)` when this message is a reply in the thread rooted at `N`.
    pub parent_id: Option<i64>,
    /// `Some(N)` when this message visually quotes the message with id `N`
    /// inline above its body. Distinct from `parent_id`: quoted messages
    /// still appear in the main timeline rather than being collapsed into
    /// a thread side panel. Always `None` for thread replies (the quote-
    /// reply affordance is suppressed inside the thread panel).
    pub quote_id: Option<i64>,
    /// True for server-authored system notices (e.g. "started a call"),
    /// which render as a centered, non-interactive line.
    pub is_system: bool,
    /// LC-74: `Some(N)` when posted by incoming webhook `N` (user_id is empty).
    pub webhook_id: Option<i64>,
    /// LC-77: `Some(N)` when posted by email-ingress inbox `N` (user_id is
    /// empty). Parallel to `webhook_id`; exactly one of the two is `Some`
    /// for any synthetic-actor message, both `None` for real-user messages.
    pub email_inbox_id: Option<i64>,
    /// LC-78: `Some(N)` when posted by protocol bridge `N` (user_id is
    /// empty). At most one of webhook_id / email_inbox_id / bridge_id is
    /// `Some` for any given message.
    pub bridge_id: Option<i64>,
    /// LC-78: snapshotted foreign display name (e.g. Matrix `alice:server`).
    /// `Some` iff `bridge_id` is `Some`. Carried on the Message broadcast so
    /// the WS render reaches the actor resolver without a re-query; same
    /// pass-through shape as `webhook_id` + the join-resolved webhook name.
    pub bridge_foreign_name: Option<String>,
    /// LC-78: snapshotted protocol kind (`matrix` / `irc` / `xmpp`). `Some`
    /// iff `bridge_id` is `Some`.
    pub bridge_kind: Option<String>,
    /// LC-78-AVATAR-PROXY: the cache key for the message's foreign avatar.
    /// `Some` when the daemon submitted a foreign avatar URL and the proxy
    /// gate was enabled at submit time. Carried on the broadcast Message so
    /// the WS render reaches the resolver without a re-query, matching the
    /// pass-through shape of `bridge_foreign_name` and `bridge_kind`.
    pub bridge_foreign_avatar: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::ephemeral_expires_at;
    use chrono::{TimeZone, Utc};

    #[test]
    fn known_ttls_map_to_absolute_expiry() {
        let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        assert_eq!(
            ephemeral_expires_at("5m", now).as_deref(),
            Some("2026-07-06 12:05:00")
        );
        assert_eq!(
            ephemeral_expires_at("1h", now).as_deref(),
            Some("2026-07-06 13:00:00")
        );
        assert_eq!(
            ephemeral_expires_at("1d", now).as_deref(),
            Some("2026-07-07 12:00:00")
        );
        assert_eq!(
            ephemeral_expires_at("7d", now).as_deref(),
            Some("2026-07-13 12:00:00")
        );
    }

    #[test]
    fn unknown_or_blank_ttl_means_permanent() {
        let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        assert_eq!(ephemeral_expires_at("", now), None);
        assert_eq!(ephemeral_expires_at("off", now), None);
        assert_eq!(ephemeral_expires_at("99y", now), None);
        // A forged value must not be honored as an arbitrary duration.
        assert_eq!(ephemeral_expires_at("100000d", now), None);
    }
}
