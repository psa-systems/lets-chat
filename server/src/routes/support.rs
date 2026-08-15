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

/// LC-716: create the dedicated support channel for a claimed ticket and return
/// `(room_id, room_name)`. The channel is a private room in the General enclave
/// (both parties are General members) joining the requester, the claiming
/// `admin`, and the assistant bot; it is seeded with a bot message carrying the
/// original request so the admin has context immediately. Both humans get a
/// per-user sidebar nudge so the room appears live (the bot needs none). It is
/// NOT broadcast to the enclave, so the private support channel stays visible
/// only to its members. The caller has already flipped the ticket to `claimed`
/// (guarded), so this runs at most once per ticket.
#[cfg(feature = "standalone")]
pub(crate) async fn claim_ticket(
    state: &AppState,
    ticket: &crate::models::support_ticket::SupportTicket,
    admin: &crate::models::User,
) -> Result<(i64, String), AppError> {
    let bot = super::assistant::assistant_bot(state).await?;
    let requester = db::auth::find_user_by_id(&state.auth, &ticket.requester_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("the requester no longer exists".into()))?;
    let requester_label = label_for(&requester);

    // Unique per ticket so a second request from the same user cannot collide on
    // the room name.
    let room_name = format!("Support: {requester_label} (#{})", ticket.id);
    let invite_code: String = {
        use rand::Rng;
        rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(10)
            .map(char::from)
            .collect()
    };
    let enclave_id = db::enclave::get_general_id(&state.chat).await?;
    let room_id = db::chat::create_room(
        &state.chat,
        &room_name,
        Some("AI help desk support request"),
        "private",
        Some(&invite_code),
        enclave_id,
    )
    .await?;
    for uid in [&ticket.requester_id, &admin.id, &bot.id] {
        db::chat::add_room_member(&state.chat, room_id, uid).await?;
    }

    // Seed the channel with the request context, posted as the assistant bot.
    let admin_label = match admin.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n,
        _ => &admin.username,
    };
    let origin = if ticket.room_name.is_empty() {
        String::new()
    } else if let Some(rid) = ticket.room_id {
        format!(" originally in [#{}](/room/{})", ticket.room_name, rid)
    } else {
        format!(" originally in #{}", ticket.room_name)
    };
    let seed = format!(
        "\u{1f198} **Support request #{}**\n\n{requester_label} asked for a human{origin}. \
         {admin_label} is now helping here.\n\n> {}",
        ticket.id, ticket.body
    );
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or_else(|| AppError::Internal("support room vanished after creation".into()))?;
    let msg_id = db::chat::insert_message(&state.chat, room_id, &bot.id, &seed).await?;
    super::room::finalize_message_send(state, &room, &bot, msg_id, &seed, None).await?;

    // Nudge both humans' sidebars so the new channel shows up without a reload.
    for uid in [&ticket.requester_id, &admin.id] {
        state.hub.broadcast_to_user(
            uid,
            &ChatEvent::RoomMemberAdded {
                room_id,
                user_id: uid.clone(),
            },
        );
    }
    Ok((room_id, room_name))
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
