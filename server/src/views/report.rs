// LC-334: message-reporting views. The modal (swapped into the singleton
// `#lc-report-modal` slot) lets a member pick a category + optional note; the
// site-admin queue and its live OOB fragment render the open-report rows from a
// shared `admin/reports_items.html` partial.
#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template;

/// One open-report row as shown in the admin queue. Built by
/// `routes::report::build_report_views` (enriches reporter/author labels from
/// the auth pool and a plain-text message excerpt from the chat pool).
pub struct ReportView {
    pub id: i64,
    pub message_id: i64,
    /// Allowlisted category string; the template maps it to a localized label.
    pub category: String,
    pub note: Option<String>,
    /// Plain-text, newline-collapsed, length-bounded excerpt of the reported
    /// message body. `(deleted)` placeholder when the message is gone.
    pub excerpt: String,
    pub reporter_label: String,
    pub author_label: String,
    /// `#room-name`, or the DM label.
    pub room_label: String,
    pub created_at: String,
}

/// The report modal (category radios + optional note), swapped into
/// `#lc-report-modal` by the hover-menu "Report" button.
#[derive(Template)]
#[template(path = "report_modal.html")]
pub struct ReportModal {
    pub message_id: i64,
}

/// Inline confirmation rendered into the modal slot after a successful report.
#[derive(Template)]
#[template(path = "report_submitted.html")]
pub struct ReportSubmitted;

/// LC-334: the nav open-count badge inner markup, fetched on load by every admin
/// page's `#admin-reports-badge` span (so the count does not need threading
/// through every admin page struct) and re-rendered live via `AdminReportsOob`.
#[derive(Template)]
#[template(path = "admin/reports_badge.html")]
pub struct AdminReportsBadge {
    pub open_count: i64,
}

/// LC-334: live OOB fragment broadcast on the `admin` topic when the open-report
/// set changes (new report / resolve / dismiss). Swaps the `#admin-reports-list`
/// region (present only on `/admin/reports`) and the `#admin-reports-badge` nav
/// count (present on every admin page); id-keyed swaps drop silently where the
/// element is absent.
#[derive(Template)]
#[template(path = "admin/reports_oob.html")]
pub struct AdminReportsOob {
    pub reports: Vec<ReportView>,
    pub open_count: i64,
}
