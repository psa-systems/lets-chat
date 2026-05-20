//! LC-63: message-reminder dispatcher. Polls `reminders` for due,
//! not-yet-fired rows, claims each atomically, and pings the owner through
//! the existing notification pipeline (on-screen toast over the WS hub +
//! Web Push) with a deep link back to the message.
//!
//! Claim-then-fire gives at-most-once delivery: `mark_fired` flips
//! `fired_at` in a single UPDATE, so an overlapping tick cannot double-send.
//! Web Push reaches the device push service regardless of whether the user
//! has a live WebSocket, so there is nothing to retry when the user is
//! merely offline; transient process death between claim and send is rare
//! and the reminder still appears (as fired) on the `/reminders` page.

use crate::state::AppState;
use crate::ws::events::ChatEvent;

#[derive(Debug, Default)]
pub struct ReminderStats {
    pub fired: usize,
}

/// One dispatch tick: fire every due reminder. Returns how many fired.
pub async fn run_reminder_tick(state: &AppState) -> sqlx::Result<ReminderStats> {
    let due = crate::db::reminders::select_due(&state.chat).await?;
    let mut stats = ReminderStats::default();
    for r in due {
        // Claim. If another tick won the race, skip.
        if !crate::db::reminders::mark_fired(&state.chat, r.id).await? {
            continue;
        }
        // Re-check access at fire time (mirrors the scheduled-message
        // re-validate-at-delivery pattern). If the user lost access to the
        // room since setting the reminder, do not push the (freshly read)
        // snippet to them. The row stays claimed, so it is not re-checked
        // every tick.
        if !accessible(state, &r).await {
            continue;
        }
        let (room_label, target_path) = link_for(state, &r).await;
        let event = ChatEvent::Reminder {
            user_id: r.user_id.clone(),
            room_id: r.room_id,
            room_type: r.room_type.clone(),
            room_label,
            message_id: r.message_id,
            snippet: r.snippet.clone(),
            target_path,
        };
        // On-screen toast for any live tabs.
        state.hub.broadcast_to_user(&r.user_id, &event);
        // Web Push (fire-and-forget; no mute check for reminders).
        let push_state = state.clone();
        let push_event = event.clone();
        let uid = r.user_id.clone();
        tokio::spawn(async move {
            crate::push::dispatch(&push_state, &uid, &push_event).await;
        });
        stats.fired += 1;
    }
    Ok(stats)
}

/// True when the reminder's owner can still see the room at fire time.
/// Resolves the owner's admin status so site admins keep god-mode access.
async fn accessible(state: &AppState, r: &crate::db::reminders::DueReminder) -> bool {
    let is_admin = matches!(
        crate::db::auth::find_user_by_id(&state.auth, &r.user_id).await,
        Ok(Some(u)) if u.role == "admin"
    );
    crate::db::chat::is_room_accessible(&state.chat, r.room_id, &r.user_id, is_admin)
        .await
        .unwrap_or(false)
}

/// Resolve the toast label and message deep link for a due reminder. DMs
/// link to `/dm/{peer}`; other rooms to `/room/{id}`. Both anchor the
/// specific message via `#msg-{id}`.
async fn link_for(state: &AppState, r: &crate::db::reminders::DueReminder) -> (String, String) {
    if r.room_type == "dm" {
        let peer = crate::db::chat::get_dm_peer(&state.chat, r.room_id, &r.user_id)
            .await
            .ok()
            .flatten();
        match peer {
            Some(peer_id) => (
                "Direct message".to_string(),
                format!("/dm/{peer_id}#msg-{}", r.message_id),
            ),
            // The user left the DM (or it is malformed); fall back to the
            // room path so the link still resolves to something.
            None => (
                "Direct message".to_string(),
                format!("/room/{}#msg-{}", r.room_id, r.message_id),
            ),
        }
    } else {
        (
            format!("#{}", r.room_name),
            format!("/room/{}#msg-{}", r.room_id, r.message_id),
        )
    }
}
