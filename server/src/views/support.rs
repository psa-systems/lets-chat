// LC-714: AI help desk support-ticket views. The site-admin queue and its live
// OOB fragment render the open tickets from a shared `admin/support_items.html`
// partial, mirroring the message-report queue (LC-334).
#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template;

/// One open support-ticket row as shown in the admin queue. Built by
/// `routes::support::build_support_views` (enriches the requester label from the
/// auth pool; `room_label` + `room_id` come denormalized off the ticket).
pub struct SupportView {
    pub id: i64,
    pub requester_label: String,
    /// `#room-name` (or a fallback) the request came from; empty hides the link.
    pub room_label: String,
    /// Origin room id for the jump link; `None` hides it.
    pub room_id: Option<i64>,
    /// The user's request text (rendered escaped, never through markdown).
    pub body: String,
    pub created_at: String,
}

/// LC-714: the nav open-count badge inner markup, fetched on load by every admin
/// page's `#admin-support-badge` span and re-rendered live via `AdminSupportOob`.
#[derive(Template)]
#[template(path = "admin/support_badge.html")]
pub struct AdminSupportBadge {
    pub open_count: i64,
}

/// LC-714: live OOB fragment broadcast on the `admin` topic when the open-ticket
/// set changes (new ticket / resolve). Swaps the `#admin-support-list` region
/// (present only on `/admin/support`) and the `#admin-support-badge` nav count
/// (present on every admin page); id-keyed swaps drop silently where the element
/// is absent.
#[derive(Template)]
#[template(path = "admin/support_oob.html")]
pub struct AdminSupportOob {
    pub tickets: Vec<SupportView>,
    pub open_count: i64,
}
