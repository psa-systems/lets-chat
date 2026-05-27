#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::db::analytics::RetentionCohort;
use crate::models::{ModAction, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// LC-97: one headline metric on the analytics dashboard: a label, the
/// most recent day's value, the sum over the selected range, and the
/// pre-rendered inline-SVG chart for that metric.
pub struct MetricCard {
    pub label: &'static str,
    pub latest: i64,
    pub total: i64,
    /// True for "stock" metrics (DAU/MAU/active rooms) where summing
    /// across days is meaningless, so the card hides the range total.
    pub is_snapshot: bool,
    pub chart_svg: String,
}

/// LC-97: admin analytics dashboard (`/admin/analytics`). All series are
/// read from the pre-aggregated `analytics_daily` table; the retention
/// triangle is computed on demand. Charts are rendered server-side as
/// inline SVG so the page ships no chart JS.
#[derive(Template)]
#[template(path = "admin/analytics.html")]
pub struct AnalyticsPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// Selected look-back window in days (7 / 30 / 90).
    pub range_days: i64,
    /// Today's UTC date, shown next to the "Recompute today" button.
    pub today: &'a str,
    pub cards: &'a [MetricCard],
    /// Column headers for the retention triangle ("W0", "W1", ...).
    pub retention_headers: &'a [String],
    pub retention: &'a [RetentionCohort],
}

/// Per-row projection for the users admin table.
pub struct AdminUserView {
    pub id: String,
    pub username: String,
    pub role: String,
    pub is_banned: bool,
    pub is_muted: bool,
    /// LC-93: pre-formatted "N.NN MiB" string of the user's current
    /// upload usage (excludes system messages).
    pub usage_display: String,
    /// LC-93: current quota as a whole-MiB string for the form input,
    /// or empty when the user is unlimited. An empty submit clears the
    /// quota.
    pub quota_mib_value: String,
}

/// Per-row projection for the invites admin table.
pub struct AdminInviteView {
    pub id: i64,
    pub code: String,
    pub created_by_username: String,
    pub used_by_username: Option<String>,
    pub created_at: String,
}

/// Per-row projection for the rooms admin table.
pub struct AdminRoomView {
    pub id: i64,
    pub name: String,
    pub topic: Option<String>,
    pub room_type: String,
    pub invite_code: Option<String>,
    pub members: i64,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/room_row.html")]
pub struct RoomRowFragment<'a> {
    pub r: &'a AdminRoomView,
    /// LC-177: when true the `<tr>` carries `hx-swap-oob="outerHTML"` so it
    /// replaces the matching `#room-{id}` row over the WebSocket. The HTTP
    /// path (the acting admin's own response) leaves it false.
    pub oob: bool,
}

/// LC-177: OOB row-removal pushed on the `admin` topic when a room is archived,
/// so every admin's room list drops the `#room-{id}` row without a reload.
#[derive(Template)]
#[template(path = "admin/room_row_delete.html")]
pub struct RoomRowDeleteFragment {
    pub room_id: i64,
}

#[derive(Template)]
#[template(path = "admin/users.html")]
pub struct UsersPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub users: &'a [AdminUserView],
}

#[derive(Template)]
#[template(path = "admin/user_row.html")]
pub struct UserRowFragment<'a> {
    pub u: &'a AdminUserView,
    /// LC-175: when true the `<tr>` carries `hx-swap-oob="outerHTML"` so it
    /// replaces the matching `#user-{id}` row over the WebSocket. The HTTP
    /// path (the acting admin's own response) leaves it false.
    pub oob: bool,
}

/// LC-175: OOB row-removal pushed on the `admin` topic when a user is deleted,
/// so every admin's user list drops the `#user-{id}` row without a reload.
#[derive(Template)]
#[template(path = "admin/user_row_delete.html")]
pub struct UserRowDeleteFragment {
    pub user_id: String,
}

#[derive(Template)]
#[template(path = "admin/rooms.html")]
pub struct RoomsPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub rooms_admin: &'a [AdminRoomView],
}

#[derive(Template)]
#[template(path = "admin/modlog.html")]
pub struct ModLogPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub entries: &'a [ModAction],
}

#[derive(Template)]
#[template(path = "admin/settings.html")]
pub struct SettingsPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// LC-77: IMAP poll configuration for email ingress.
    /// `imap_password_configured` is `true` when a non-empty sealed password
    /// is stored, so the template can show "configured" without ever
    /// rendering the plaintext.
    pub imap_host: String,
    pub imap_port: String,
    pub imap_tls: bool,
    pub imap_username: String,
    pub imap_password_configured: bool,
    pub imap_folder: String,
    pub imap_ingress_domain: String,
    pub imap_dead_letter_folder: String,
    pub imap_enabled: bool,
    /// `Some(msg)` when an IMAP form save just failed (typically because the
    /// secret key is unset and the password can't be sealed).
    pub imap_error: Option<String>,
    /// `true` when the IMAP form just saved successfully.
    pub imap_saved: bool,
    /// Whether newly-registered users start with the email-digest
    /// preference flipped on. Stored as a string "0" / "1" in the
    /// `settings` table under `default_notify_email_digest`.
    pub default_notify_email_digest: bool,
    /// Current value of the maintenance-mode toggle.
    pub maintenance_enabled: bool,
    /// Operator-facing message shown on the 503 page while maintenance
    /// is on. Empty hides the message block.
    pub maintenance_message: String,
    /// Pre-formatted "N.NN MiB" string; Askama can't do the i64-to-f64
    /// arithmetic inline, so the handler renders it.
    pub uploads_total_display: String,
    pub uploads_orphan_count: i64,
    /// Flash counts populated from the post-redirect query string.
    /// Some(N) renders a one-line success banner; None renders nothing.
    pub regenerated: Option<i64>,
    pub purged: Option<i64>,
}

pub struct AdminEnclaveView {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub invite_code: Option<String>,
    pub member_count: i64,
    pub owner_id: Option<String>,
    pub created_at: String,
    /// LC-93: same formatting convention as `AdminUserView`.
    pub usage_display: String,
    pub quota_mib_value: String,
}

#[derive(Template)]
#[template(path = "admin/enclaves.html")]
pub struct EnclavesPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub enclaves: &'a [AdminEnclaveView],
}

#[derive(Template)]
#[template(path = "admin/invites.html")]
pub struct InvitesPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub invites: &'a [AdminInviteView],
}

// LC-94 anti-spam pages ----------------------------------------------------

#[derive(Template)]
#[template(path = "admin/anti_spam.html")]
pub struct AntiSpamPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub rate_limit_messages: u32,
    pub rate_limit_registrations: u32,
    pub rate_limit_logins: u32,
    pub rate_limit_password_resets: u32,
    pub link_filter_enabled: bool,
    pub honeypot_enabled: bool,
    pub saved: bool,
}

pub struct LinkFilterRuleView {
    pub id: i64,
    pub pattern: String,
    pub action: String,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/link_filter.html")]
pub struct LinkFilterPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub rules: &'a [LinkFilterRuleView],
    pub error: Option<String>,
}

pub struct QuarantineEntryView {
    pub message_id: i64,
    pub room_id: i64,
    pub author_id: String,
    pub body: String,
    pub matched_pattern: String,
    pub matched_url: String,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/quarantine.html")]
pub struct QuarantinePage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub entries: &'a [QuarantineEntryView],
}

// LC-95 backup / restore ----------------------------------------------------

#[derive(Template)]
#[template(path = "admin/backup_restore.html")]
pub struct BackupRestorePage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// True when a `.restore-pending` marker exists in the data dir;
    /// the page shows a banner instructing the operator to restart.
    pub restore_pending: bool,
    /// Inline error from a failed stage attempt; rendered above the
    /// upload form on the retry.
    pub error: Option<String>,
}

// LC-96 branding admin page -----------------------------------------------

#[derive(Template)]
#[template(path = "admin/branding.html")]
pub struct BrandingPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub primary_color: String,
    pub accent_color: String,
    pub login_heading: String,
    pub login_body: String,
    pub has_logo: bool,
    /// LC-142: whether a custom global favicon is set (drives the preview
    /// and the "current favicon" hint on the admin form).
    pub has_favicon: bool,
    pub saved: bool,
    pub error: Option<String>,
}

/// LC-76: one custom slash command row on the admin page.
pub struct SlashCommandRowView {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub target: String,
    pub admin_only: bool,
}

/// LC-76: a built-in command, listed read-only for operator reference.
pub struct BuiltinCommandRowView {
    pub usage: String,
    pub description: String,
}

#[derive(Template)]
#[template(path = "admin/slash_commands.html")]
pub struct SlashCommandsPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub builtins: &'a [BuiltinCommandRowView],
    pub commands: &'a [SlashCommandRowView],
    pub error: Option<String>,
}

/// LC-73: one bot row on the admin bots page.
pub struct BotRowView {
    pub id: String,
    pub username: String,
    pub disabled: bool,
    pub created_at: String,
}

/// LC-78: one bridge row on the admin bridges page.
pub struct BridgeRowView {
    pub id: i64,
    pub room_id: i64,
    pub room_name: String,
    pub kind: String,
    pub bot_username: String,
    /// Derived from `last_heartbeat_at` age + DB status. One of
    /// `pending` / `healthy` / `stale` / `errored`.
    pub status: &'static str,
    pub last_heartbeat_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/bridges.html")]
pub struct BridgesPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// True when LETS_CHAT_SECRET_KEY is set: bridges store sealed config
    /// and mint API tokens, both of which require the key.
    pub available: bool,
    pub bridges: &'a [BridgeRowView],
    pub rooms: &'a [AdminRoomView],
    /// Plaintext token for a just-created bridge bot, shown exactly once.
    pub new_token: Option<String>,
    pub new_bot_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/bots.html")]
pub struct BotsPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// True when the server has a secret key (required to mint the bot's API token).
    pub available: bool,
    pub all_scopes: &'a [&'a str],
    pub bots: &'a [BotRowView],
    /// Plaintext API token for a just-created bot, shown exactly once.
    pub new_token: Option<String>,
    /// The just-created bot's username, paired with new_token.
    pub new_bot_name: Option<String>,
    pub error: Option<String>,
}

// LC-75: outgoing webhooks ------------------------------------------------

/// All event names a subscription may select, for the create-form checkboxes.
pub const OUTGOING_EVENTS: &[&str] = &[
    "message.posted",
    "message.edited",
    "message.deleted",
    "reaction.added",
];

pub struct OutgoingWebhookRowView {
    pub id: i64,
    pub scope: String,
    pub events: String,
    pub url: String,
    pub created_at: String,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub consecutive_failures: i64,
    pub disabled: bool,
}

#[derive(Template)]
#[template(path = "admin/outgoing_webhooks.html")]
pub struct OutgoingWebhooksPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub all_events: &'a [&'a str],
    pub webhooks: &'a [OutgoingWebhookRowView],
    /// The signing secret of a just-created / just-rotated webhook, with the
    /// id it belongs to, shown once.
    pub revealed: Option<(i64, String)>,
    pub error: Option<String>,
}

pub struct DeliveryRowView {
    pub id: i64,
    pub event: String,
    pub attempt: i64,
    pub status: Option<i64>,
    pub scheduled_at: String,
    pub delivered_at: Option<String>,
}

impl DeliveryRowView {
    /// "ok" (2xx) | "neterr" (0) | "warn" (other code) | "pending" (NULL).
    /// Keeps the integer comparison out of the template (Askama compares
    /// `&i64` awkwardly).
    pub fn status_kind(&self) -> &'static str {
        match self.status {
            Some(s) if (200..300).contains(&s) => "ok",
            Some(0) => "neterr",
            Some(_) => "warn",
            None => "pending",
        }
    }

    /// Human display of the status column.
    pub fn status_display(&self) -> String {
        match self.status {
            Some(0) => "network error".to_string(),
            Some(s) => s.to_string(),
            None => "pending".to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "admin/outgoing_webhook_deliveries.html")]
pub struct OutgoingWebhookDeliveriesPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub webhook_id: i64,
    pub deliveries: &'a [DeliveryRowView],
}
