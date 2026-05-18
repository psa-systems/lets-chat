use askama::Template;

/// One row in the autocomplete dropdown rendered by
/// `partials/mention_popover.html`. The `kind` discriminator drives the
/// template's branch between a normal user row (avatar + username +
/// optional display name) and a broadcast row (`@`-glyph badge + token +
/// subtitle). Broadcast suggestions reuse the user-shaped fields by
/// convention: `username` carries the token name (`"here"` / `"channel"`)
/// so the composer's `data-username` insert path works unchanged;
/// `user_id`, `display_name`, and `avatar_ext` are empty / None for
/// broadcast rows.
#[derive(Clone)]
pub struct MentionSuggestion {
    pub kind: &'static str,
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    /// Short explainer rendered next to the token on a broadcast row, e.g.
    /// "Notify online members." None for user rows.
    pub subtitle: Option<&'static str>,
}

impl MentionSuggestion {
    /// Convenience constructor for a user-kind suggestion. Used by the
    /// autocomplete handler so the existing call sites are not littered
    /// with `kind: "user"` and `subtitle: None`.
    pub fn user(
        user_id: String,
        username: String,
        display_name: Option<String>,
        avatar_ext: Option<String>,
    ) -> Self {
        Self {
            kind: "user",
            user_id,
            username,
            display_name,
            avatar_ext,
            subtitle: None,
        }
    }

    /// Convenience constructor for a broadcast-kind suggestion (`@here` or
    /// `@channel`). The `username` is the token name; the composer reads
    /// it from `data-username` and inserts `@{token}` into the textarea.
    pub fn broadcast(token: &'static str, subtitle: &'static str) -> Self {
        Self {
            kind: "broadcast",
            user_id: String::new(),
            username: token.to_string(),
            display_name: None,
            avatar_ext: None,
            subtitle: Some(subtitle),
        }
    }

    /// Convenience constructor for a user-group suggestion. The `username`
    /// is the group name; the composer inserts `@{name}` into the
    /// textarea exactly like a user mention. `display_name` carries a
    /// rendered "N members" label for the secondary text in the row.
    pub fn group(name: String, member_count: i64) -> Self {
        let label = if member_count == 1 {
            "1 member".to_string()
        } else {
            format!("{member_count} members")
        };
        Self {
            kind: "group",
            user_id: String::new(),
            username: name,
            display_name: Some(label),
            avatar_ext: None,
            subtitle: None,
        }
    }
}

#[derive(Template)]
#[template(path = "partials/mention_popover.html")]
pub struct MentionPopoverFragment<'a> {
    pub results: &'a [MentionSuggestion],
}

/// Composer-slot fragment rendered by `GET /api/rooms/:room_id/broadcast-count`.
/// `count` is the number of recipients who would receive the broadcast if
/// the message were sent right now (post mute / DND filtering). `room_name`
/// is included in the visible label so the user knows which room they're
/// about to ping. `count == 0` is handled by the template (empty body so
/// the slot collapses visually while keeping its `min-h-[1rem]` reservation).
#[derive(Template)]
#[template(path = "partials/broadcast_count.html")]
pub struct BroadcastCountFragment<'a> {
    pub count: i64,
    pub room_name: &'a str,
}
