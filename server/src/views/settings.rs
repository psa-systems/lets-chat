use askama::Template;

#[allow(unused_imports)]
use crate::i18n::filters; // LC-100: in-scope for the `| t` filter in templates.
use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row in the sessions list on the settings page.
pub struct SessionView {
    pub id: String,
    pub label: String,
    pub ip: Option<String>,
    pub last_seen: String,
    pub created: String,
    pub is_current: bool,
}

#[derive(Template)]
#[template(path = "settings/page.html")]
pub struct UserSettingsPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    /// LC-347: cache-buster for the avatar preview `<img>`. Unlike
    /// `asset_version` (a build constant) this changes whenever the avatar
    /// file changes, so the new image loads immediately after an upload
    /// instead of being masked by the avatar route's `max-age=300`.
    pub avatar_version: String,
    pub saved: bool,
    /// LC-356: inline error flash for the profile / delete-account forms
    /// (set via `?error=` by `settings_error_redirect`).
    pub error: Option<String>,
    pub push_available: bool,
    pub email: Option<String>,
    /// Mirror of `state.mail_available()`. Drives the disabled state of
    /// the email-digest checkbox and the help-text branch that explains
    /// why opting in is currently a no-op.
    pub email_available: bool,
    /// Live sessions for this user, sorted newest activity first. The row
    /// matching the request's session cookie has `is_current = true` so the
    /// template can mark it and disable its revoke button.
    pub sessions: &'a [SessionView],
    /// Flash set by `?session_revoked=1` after a successful revoke.
    pub session_revoked: bool,
    /// LC-93: pre-formatted "N.NN MiB" of the user's current upload
    /// usage. Always populated; an unlimited account renders the
    /// usage with an "(unlimited)" tail label.
    pub storage_usage_display: String,
    /// LC-93: `Some("N.NN MiB")` when the user has a cap, `None` when
    /// they are unlimited.
    pub storage_quota_display: Option<String>,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    /// LC-88: Do Not Disturb. `dnd_active` drives the "currently quiet"
    /// banner; `dnd_paused_until` is the raw stored instant (empty when no
    /// manual pause). The five schedule fields are pre-split from the stored
    /// JSON so the form renders the user's current quiet hours. `timezones`
    /// is the IANA name list for the timezone picker.
    pub dnd_active: bool,
    pub dnd_paused_until: String,
    pub dnd_timezone: String,
    pub dnd_weekday_start: String,
    pub dnd_weekday_end: String,
    pub dnd_weekend_start: String,
    pub dnd_weekend_end: String,
    pub timezones: Vec<TzOption>,
    /// LC-100: available UI locales for the language picker, plus a synthetic
    /// "system" option (empty code) for the Accept-Language fallback.
    pub locales: Vec<LocaleOption>,
    /// LC-304: the user's highlight words, alphabetical, for the chip list.
    pub keywords: Vec<String>,
    /// LC-482: the user's personal custom emoji, for the Custom-emoji panel.
    pub personal_emojis: Vec<crate::models::custom_emoji::CustomEmoji>,
    /// LC-482: per-emoji byte cap, surfaced to the upload form help text.
    pub emoji_max_kib: i64,
    /// LC-487: the user's canned responses (saved replies), name-ordered.
    /// `target` carries the body; `description` the optional help line.
    pub canned_responses: Vec<crate::db::slash::CustomCommand>,
}

/// LC-426: reusable feedback fragment returned by the settings form handlers
/// for htmx requests. The main content is the inline per-form status (swapped
/// into the form's status slot); an out-of-band block appends a toast into
/// `#lc-toast-region`. `toast_only` suppresses the inline status (used by the
/// session-revoke path, whose target is the row it removes). `reset_avatar`
/// adds an OOB swap that reverts the avatar preview to the letter fallback
/// after a successful "Remove avatar".
#[derive(Template)]
#[template(path = "settings/feedback.html")]
pub struct SettingsFeedback {
    pub ok: bool,
    pub message: String,
    pub toast_only: bool,
    pub reset_avatar: bool,
    /// LC-432: when `reset_avatar`, the `/avatars/{id}?v=...` URL the OOB swap
    /// points the single preview `<img>` back to (the generated default).
    pub avatar_src: String,
}

impl SettingsFeedback {
    pub fn ok(message: String) -> Self {
        Self {
            ok: true,
            message,
            toast_only: false,
            reset_avatar: false,
            avatar_src: String::new(),
        }
    }
    pub fn err(message: String) -> Self {
        Self {
            ok: false,
            message,
            toast_only: false,
            reset_avatar: false,
            avatar_src: String::new(),
        }
    }
    pub fn toast_only_ok(message: String) -> Self {
        Self {
            ok: true,
            message,
            toast_only: true,
            reset_avatar: false,
            avatar_src: String::new(),
        }
    }
    pub fn ok_reset_avatar(message: String, avatar_src: String) -> Self {
        Self {
            ok: true,
            message,
            toast_only: false,
            reset_avatar: true,
            avatar_src,
        }
    }
}

/// LC-304: the highlight-words chip list, re-rendered by the add/remove
/// endpoints and included by the settings page.
#[derive(Template)]
#[template(path = "partials/keyword_list.html")]
pub struct KeywordListFragment {
    pub keywords: Vec<String>,
    /// True when the per-user cap is reached (the add input is disabled).
    pub at_cap: bool,
}

/// One entry in the language picker. `selected` is precomputed; an empty
/// `code` is the "use browser language" (clear preference) option.
pub struct LocaleOption {
    pub code: String,
    pub name: String,
    pub selected: bool,
}

/// One entry in the DND timezone picker. `selected` is precomputed so the
/// template needs no string comparison (Askama can't compare `String` to the
/// `&&str` yielded by iterating a `&[&str]`).
pub struct TzOption {
    pub name: &'static str,
    pub selected: bool,
}

pub struct BlockedUserView {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
}

impl BlockedUserView {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
}

#[derive(Template)]
#[template(path = "settings/blocked.html")]
pub struct BlockedListPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub blocked: &'a [BlockedUserView],
    pub error: Option<&'a str>,
    pub form_username: &'a str,
}
