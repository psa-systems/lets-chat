#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Attachment, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row on the Saved page. Pre-resolved by the route handler so the
/// template stays free of business logic. `context_path` is the URL the
/// "in #room" / "in @peer" link points to.
pub struct SavedListRow {
    pub message_id: i64,
    pub author_label: String,
    /// LC-684: the body run through the same `views::markdown` pipeline the
    /// timeline uses (sanitized HTML), rather than raw source. Empty when the
    /// saved message has no text body (e.g. an image-only or file message).
    pub body_html: String,
    /// LC-684: the saved message's attachments, rendered read-only the same way
    /// the timeline renders them (image / video / voice / file card).
    pub attachments: Vec<Attachment>,
    pub message_created_at: String,
    pub saved_at: String,
    pub context_label: String,
    pub context_path: String,
    /// LC-479: user-set label ("folder"); None = unlabeled.
    pub label: Option<String>,
}

impl SavedListRow {
    /// LC-479: label text for the inline editor `value` / row `data-` attr,
    /// "" when unlabeled. Keeps the template free of Option matching.
    pub fn label_value(&self) -> &str {
        self.label.as_deref().unwrap_or("")
    }

    /// LC-684: true when the row has something to show (rendered text or at
    /// least one attachment). Drives the "No preview available" placeholder so a
    /// saved message never renders as a blank card.
    pub fn has_content(&self) -> bool {
        !self.body_html.trim().is_empty() || !self.attachments.is_empty()
    }
}

/// LC-479: distinct non-empty labels in use across `rows`, sorted, for the
/// /saved filter chips. Shared by `SavedPage` and `SavedListFragment` so the
/// page and the live OOB refresh render the same chip set.
fn distinct_labels(rows: &[SavedListRow]) -> Vec<String> {
    let mut seen: Vec<String> = rows
        .iter()
        .filter_map(|r| r.label.as_deref())
        .map(|l| l.to_string())
        .collect::<std::collections::HashSet<String>>()
        .into_iter()
        .collect();
    seen.sort_unstable();
    seen
}

/// LC-479: whether any row is unlabeled (drives the "Unlabeled" filter chip).
fn any_unlabeled(rows: &[SavedListRow]) -> bool {
    rows.iter().any(|r| r.label.is_none())
}

/// LC-178: OOB swap of the /saved list region (`#lc-saved-list`) after a
/// bookmark/unbookmark/label change, so every tab refreshes without a reload.
/// Shares `saved/items.html` with `SavedPage`.
#[derive(Template)]
#[template(path = "ws/saved_live.html")]
pub struct SavedListFragment<'a> {
    pub entries: &'a [SavedListRow],
}

impl SavedListFragment<'_> {
    /// LC-479: see `distinct_labels`. Called from the shared `saved/items.html`.
    pub fn labels(&self) -> Vec<String> {
        distinct_labels(self.entries)
    }
    /// LC-479: see `any_unlabeled`.
    pub fn has_unlabeled(&self) -> bool {
        any_unlabeled(self.entries)
    }
}

#[derive(Template)]
#[template(path = "saved/page.html")]
pub struct SavedPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub entries: &'a [SavedListRow],
    pub asset_version: &'a str,
}

impl SavedPage<'_> {
    /// LC-479: see `distinct_labels`. Called from the shared `saved/items.html`.
    pub fn labels(&self) -> Vec<String> {
        distinct_labels(self.entries)
    }
    /// LC-479: see `any_unlabeled`.
    pub fn has_unlabeled(&self) -> bool {
        any_unlabeled(self.entries)
    }
}
