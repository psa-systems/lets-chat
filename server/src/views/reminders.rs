use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row on the `/reminders` page. Pre-resolved by the handler so the
/// template stays presentation-only. `context_path` deep-links to the
/// reminded message; `fired_at` is `Some` for the "recently fired" list.
pub struct ReminderListRow {
    pub id: i64,
    pub snippet: String,
    pub remind_at: String,
    pub fired_at: Option<String>,
    pub context_label: String,
    pub context_path: String,
}

#[derive(Template)]
#[template(path = "reminders/page.html")]
pub struct RemindersPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub pending: &'a [ReminderListRow],
    pub fired: &'a [ReminderListRow],
    pub asset_version: &'a str,
}

/// The "Remind me" picker, swapped into the singleton `#lc-reminder-modal`
/// slot by the message hover-menu button. Posts to `POST /reminders`.
#[derive(Template)]
#[template(path = "reminders/picker.html")]
pub struct ReminderPicker {
    pub message_id: i64,
}

/// Inline confirmation rendered into the picker slot after a successful
/// `POST /reminders`. Auto-dismisses client-side.
#[derive(Template)]
#[template(path = "reminders/confirm.html")]
pub struct ReminderConfirm {
    pub remind_at: String,
}
