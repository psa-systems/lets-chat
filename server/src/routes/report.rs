//! LC-334: message reporting. A hover-menu "Report" action opens a modal where a
//! member picks a category + optional note; site admins triage the open queue at
//! `/admin/reports`. The modal + submit handlers live here (compiled in BOTH
//! build modes); the review queue + resolve/dismiss live in `routes::admin`
//! (standalone-only). Filing, resolving, or dismissing a report broadcasts an
//! `AdminReportChanged` signal on the `admin` topic so every admin tab updates.

use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::report::{ReportModal, ReportSubmitted};
#[cfg(feature = "standalone")]
use crate::views::report::{AdminReportsOob, ReportView};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// `display_name` if non-empty, else `username`. Only used by the admin
/// review-queue helpers below, which are `standalone`-only.
#[cfg(feature = "standalone")]
fn label_for(rec: &crate::models::user::UserRecord) -> String {
    match rec.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => rec.username.clone(),
    }
}

/// Plain-text, whitespace-collapsed, length-bounded excerpt for the queue
/// row. Only used by the admin review-queue helpers below, which are
/// `standalone`-only.
#[cfg(feature = "standalone")]
fn excerpt(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(140).collect();
    if collapsed.chars().count() > 140 {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[derive(Deserialize)]
pub struct ReportForm {
    pub category: String,
    /// Optional free-text note; an empty submission is normalized to `None`.
    pub note: Option<String>,
}

/// `GET /messages/{message_id}/report`
///
/// Render the report modal. Access to the message is gated (a viewer can only
/// report something they can see); the POST re-checks every gate.
pub async fn get_report_modal(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    let msg = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !db::chat::is_room_accessible(&state.chat, msg.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    html(&ReportModal { message_id })
}

/// `POST /messages/{message_id}/report`
///
/// File a report. Validates the category against the allowlist and the note
/// against `MAX_REPORT_NOTE_CHARS`, gates message access, and inserts
/// idempotently (a repeat report by the same user is a silent no-op). On a
/// newly-created report, broadcasts `AdminReportChanged` so admin queues update.
pub async fn post_report(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    Form(form): Form<ReportForm>,
) -> Result<Html, AppError> {
    if !db::reports::is_valid_category(&form.category) {
        return Err(AppError::BadRequest("invalid report category".into()));
    }
    let note = form
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(n) = note {
        if n.chars().count() > db::reports::MAX_REPORT_NOTE_CHARS {
            return Err(AppError::BadRequest(format!(
                "note too long (max {} characters)",
                db::reports::MAX_REPORT_NOTE_CHARS
            )));
        }
    }

    let is_admin = user.role == "admin";
    let msg = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !db::chat::is_room_accessible(&state.chat, msg.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let created = db::reports::create(
        &state.chat,
        message_id,
        msg.room_id,
        &user.id,
        &form.category,
        note,
    )
    .await?;
    if created {
        broadcast_reports_changed(&state);
    }
    // Same confirmation whether or not a duplicate suppressed the insert: the
    // reporter should not learn that they (or others) reported it before.
    html(&ReportSubmitted)
}

/// Broadcast the report-set-changed signal to the `admin` topic. The WS send
/// task re-queries and renders the OOB fragment per admin.
pub(crate) fn broadcast_reports_changed(state: &AppState) {
    state
        .hub
        .broadcast_to_topic("admin", &ChatEvent::AdminReportChanged);
}

/// Build the enriched open-report rows for the queue: reporter + author display
/// labels (auth pool) and a plain-text message excerpt (chat pool). A
/// soft-deleted / vanished message renders a placeholder excerpt rather than
/// dropping the report. Shared by the page handler, the resolve/dismiss
/// responses, and the WS OOB render.
#[cfg(feature = "standalone")]
pub(crate) async fn build_report_views(state: &AppState) -> Result<Vec<ReportView>, AppError> {
    let reports = db::reports::list_open(&state.chat).await?;
    let mut views = Vec::with_capacity(reports.len());
    for r in reports {
        let (excerpt_text, author_label) =
            match db::chat::get_message(&state.chat, r.message_id).await? {
                Some(msg) => {
                    let author = match db::auth::find_user_by_id(&state.auth, &msg.user_id).await? {
                        Some(rec) => label_for(&rec),
                        None => "(unknown)".to_string(),
                    };
                    (excerpt(&msg.body), author)
                }
                None => (
                    crate::i18n::translate_current("report-message-deleted"),
                    "(unknown)".to_string(),
                ),
            };
        let reporter_label = match db::auth::find_user_by_id(&state.auth, &r.reporter_id).await? {
            Some(rec) => label_for(&rec),
            None => "(unknown)".to_string(),
        };
        let room_label = match db::chat::get_room(&state.chat, r.room_id).await? {
            Some(room) if room.room_type == "dm" => {
                crate::i18n::translate_current("report-room-dm")
            }
            Some(room) => format!("#{}", room.name),
            None => crate::i18n::translate_current("report-room-dm"),
        };
        views.push(ReportView {
            id: r.id,
            message_id: r.message_id,
            category: r.category,
            note: r.note,
            excerpt: excerpt_text,
            reporter_label,
            author_label,
            room_label,
            created_at: r.created_at,
        });
    }
    Ok(views)
}

/// Render the report-queue OOB fragment (`#admin-reports-list` + nav badge) for
/// the acting admin's HTTP response. The same change is broadcast to the topic
/// for every other admin tab.
#[cfg(feature = "standalone")]
pub(crate) async fn render_reports_oob(state: &AppState) -> Result<Html, AppError> {
    let reports = build_report_views(state).await?;
    let open_count = reports.len() as i64;
    html(&AdminReportsOob {
        reports,
        open_count,
    })
}
