//! LC-611: telling a room's members that a huddle started.
//!
//! Starting a huddle used to broadcast one `ChatEvent::VoiceJoined` over
//! `broadcast_to_room`, which reaches only sockets already subscribed to that
//! room - i.e. the people who can see the huddle bar anyway. Members looking
//! anywhere else in the app learned nothing, so an ongoing group call was
//! something you discovered rather than something you were invited to.
//!
//! This module holds the fan-out and, more importantly, the policy around it:
//! who gets rung, when, and who does not. That is domain logic rather than
//! transport, which is why it does not live in `routes::ws`.
//!
//! Deliberately NOT built on `Hub::try_start_ringing`. That slot arbitrates DM
//! glare - two peers inviting each other at once, where exactly one must win.
//! A huddle has no such contest: two people starting one in the same room
//! simultaneously both land in the same `voice_rooms` entry and are already in
//! a call together. Reusing the slot would have meant inventing a loser for a
//! race with no stakes, and would have put the 1:1 call path (which currently
//! has no test coverage) at risk for nothing.

use crate::db;
use crate::models::User;
use crate::state::AppState;
use crate::ws::events::ChatEvent;

/// Ring the members of `room_id` that `starter` just began a huddle.
///
/// `started` is whether this join found the mesh empty. A huddle rings when it
/// starts, not every time somebody arrives, so the fourth person joining an
/// ongoing call does not re-notify everyone who already ignored it.
pub async fn ring_members(
    state: &AppState,
    starter: &User,
    starter_name: &str,
    room_id: i64,
    started: bool,
) {
    if !started {
        return;
    }
    let Ok(Some(room)) = db::chat::get_room(&state.chat, room_id).await else {
        return;
    };
    // A persistent enclave voice channel is a place people go to deliberately;
    // the first person through the door has not summoned anybody. Only the
    // ad-hoc huddle attached to a text room rings.
    if room.is_voice {
        return;
    }
    let Ok(members) = db::chat::list_room_member_ids(&state.chat, room_id).await else {
        return;
    };
    for member in members {
        if member == starter.id {
            continue;
        }
        let mode = db::notifications::room_mute_mode(&state.chat, &member, room_id)
            .await
            .unwrap_or(db::notifications::MuteMode::None);
        if !mode.allows_huddle_ring() {
            continue;
        }
        state.hub.broadcast_to_user(
            &member,
            &ChatEvent::HuddleStarted {
                room_id,
                room_name: room.name.clone(),
                to_user_id: member.clone(),
                starter_id: starter.id.clone(),
                starter_name: starter_name.to_string(),
            },
        );
    }
}
