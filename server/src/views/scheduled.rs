//! LC-62: view structs for the `/scheduled` management page and its
//! row-level HTMX fragments.

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One pending scheduled message as rendered on `/scheduled`. The body
/// is pre-rendered to HTML through `views::markdown::render` against the
/// current users table, so a mention chip whose target was renamed or
/// deleted between schedule and delivery falls back to literal text and
/// the author sees the chip-vs-text outcome before delivery (the Option
/// B visible signal from the brainstorm).
pub struct PendingRow {
    pub id: i64,
    pub room_label: String,
    pub room_path: String,
    pub scheduled_for: String,
    /// Already-sanitized HTML from `views::markdown::render`.
    pub body_html: String,
    /// LC-485: recurrence kind (`none` / `daily` / `weekly` / `weekdays`).
    /// `none` renders no badge; others render a "Repeats ..." label.
    pub repeat: String,
}

/// One dropped scheduled message as rendered on `/scheduled`'s "Recently
/// dropped" section. Surfaces `failed_reason` so the author sees why
/// their scheduled message did not deliver.
pub struct DroppedRow {
    pub id: i64,
    pub room_label: String,
    pub scheduled_for: String,
    pub delivered_at: String,
    pub failed_reason: String,
    pub body_html: String,
}

#[derive(Template)]
#[template(path = "scheduled.html")]
pub struct ScheduledPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub pending: &'a [PendingRow],
    pub dropped: &'a [DroppedRow],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "scheduled_row.html")]
pub struct ScheduledRowFragment<'a> {
    pub row: &'a PendingRow,
}
