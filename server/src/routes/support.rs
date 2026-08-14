//! LC-714: AI help desk support tickets. A ticket is filed by `/human`
//! (`routes::help_docs`) when a user asks for a person and no admin is available;
//! site admins triage the open queue at `/admin/support`. The broadcast helper
//! is compiled in BOTH build modes (like `routes::report`); the queue-render
//! helpers are standalone-only (the admin queue is `#[cfg(standalone)]`).
//! Filing or resolving a ticket broadcasts `AdminSupportChanged` on the `admin`
//! topic so every admin tab updates.

use crate::state::AppState;
use crate::ws::events::ChatEvent;

#[cfg(feature = "standalone")]
use crate::db;
#[cfg(feature = "standalone")]
use crate::error::AppError;
#[cfg(feature = "standalone")]
use crate::views::support::{AdminSupportOob, SupportView};
#[cfg(feature = "standalone")]
use crate::views::{html, Html};

/// `display_name` if non-empty, else `username`. Only used by the standalone
/// queue helpers below.
#[cfg(feature = "standalone")]
fn label_for(rec: &crate::models::user::UserRecord) -> String {
    match rec.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => rec.username.clone(),
    }
}

/// Broadcast the ticket-set-changed signal to the `admin` topic. The WS send
/// task re-queries and renders the OOB fragment per admin.
pub(crate) fn broadcast_support_changed(state: &AppState) {
    state
        .hub
        .broadcast_to_topic("admin", &ChatEvent::AdminSupportChanged);
}

/// Build the enriched open-ticket rows for the queue: the requester display
/// label (auth pool) and a room label from the ticket's denormalized room name.
/// Shared by the page handler, the resolve response, and the WS OOB render.
#[cfg(feature = "standalone")]
pub(crate) async fn build_support_views(state: &AppState) -> Result<Vec<SupportView>, AppError> {
    let tickets = db::support_tickets::list_open(&state.chat).await?;
    let mut views = Vec::with_capacity(tickets.len());
    for t in tickets {
        let requester_label = match db::auth::find_user_by_id(&state.auth, &t.requester_id).await? {
            Some(rec) => label_for(&rec),
            None => "(unknown)".to_string(),
        };
        let room_label = if t.room_name.is_empty() {
            String::new()
        } else {
            format!("#{}", t.room_name)
        };
        views.push(SupportView {
            id: t.id,
            requester_label,
            room_label,
            room_id: t.room_id,
            body: t.body,
            created_at: t.created_at,
        });
    }
    Ok(views)
}

/// Render the ticket-queue OOB fragment (`#admin-support-list` + nav badge) for
/// the acting admin's HTTP response. The same change is broadcast to the topic
/// for every other admin tab.
#[cfg(feature = "standalone")]
pub(crate) async fn render_support_oob(state: &AppState) -> Result<Html, AppError> {
    let tickets = build_support_views(state).await?;
    let open_count = tickets.len() as i64;
    html(&AdminSupportOob {
        tickets,
        open_count,
    })
}
