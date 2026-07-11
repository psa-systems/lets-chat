use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use askama::Template;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::auth::OptionalUser;
use crate::db;
use crate::models::{self, User};
use crate::state::AppState;
use crate::views::message_actor::MessageActor;
use crate::views::render_template;
use crate::views::room::ReplyCountFragment;
use crate::views::room::{MessageView, ReactionView};
use crate::views::ws_fragments::{
    render_event, AckUpdateFragment, CallSignalFragment, EditedMessageFragment,
    MentionClearedFragment, MentionedFragment, NewMessageFragment, ReactionUpdateFragment,
    ReminderFragment, SeenIndicatorFragment, SidebarUpdateFragment, ThreadReplyOobFragment,
    TranscriptControlFragment, TranscriptSegmentFragment, UnreadBadgeFragment, VoiceEventFragment,
};
use crate::ws::events::ChatEvent;
use crate::ws::hub::{ConnId, RingingResult};

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientFrame {
    #[serde(rename = "subscribe")]
    Subscribe { room_id: i64 },
    /// LC-160: subscribe to a typed live-update topic ("enclave:{id}",
    /// "user:{id}", "admin"). Authorized per kind before the connection joins
    /// the topic's fan-out set.
    #[serde(rename = "subscribe_topic")]
    SubscribeTopic { topic: String },
    #[serde(rename = "typing")]
    Typing { room_id: i64 },
    #[serde(rename = "thread_typing")]
    ThreadTyping { room_id: i64, parent_id: i64 },
    /// LC-498: heartbeat sent while a user has the room wiki editor focused.
    /// Drives the "X is editing the wiki" presence banner (mirrors `typing`).
    #[serde(rename = "wiki_editing")]
    WikiEditing { room_id: i64 },
    /// WebRTC 1:1 call signaling. Relayed verbatim to the other member of
    /// the DM room; the server validates membership and never inspects
    /// `payload`. See [`ChatEvent::CallSignal`].
    #[serde(rename = "call_signal")]
    CallSignal {
        room_id: i64,
        kind: String,
        #[serde(default)]
        payload: Option<String>,
    },
    /// LC-183: remote-control consent handshake. `kind` is
    /// request/grant/deny/revoke; relayed to the DM peer after the
    /// verified-email + block gate. No payload (input rides a data channel).
    #[serde(rename = "remote_control_signal")]
    RemoteControlSignal { room_id: i64, kind: String },
    /// Join an enclave voice channel (`room_type = 'voice'`).
    #[serde(rename = "voice_join")]
    VoiceJoin { room_id: i64 },
    /// Leave the voice channel this connection is in.
    #[serde(rename = "voice_leave")]
    VoiceLeave { room_id: i64 },
    /// Mesh signaling to a specific peer in a voice channel. See
    /// [`ChatEvent::VoiceSignal`].
    #[serde(rename = "voice_signal")]
    VoiceSignal {
        room_id: i64,
        target_user_id: String,
        kind: String,
        #[serde(default)]
        payload: Option<String>,
    },
    /// LC-402: a voice participant toggled their mic; broadcast to the channel
    /// so peers can show the mute indicator on that participant's tile.
    #[serde(rename = "voice_mute")]
    VoiceMute { room_id: i64, muted: bool },
    /// LC-408: a voice participant started/stopped screen-sharing; broadcast so
    /// peers can pin that participant's tile to the stage.
    #[serde(rename = "voice_screen")]
    VoiceScreen { room_id: i64, sharing: bool },
    /// LC-494: stage control plane. Join/leave as a listener, request or
    /// withdraw the floor, step down from speaking, and (host-only) grant or
    /// revoke another participant's floor. Each mutates the ephemeral hub
    /// roster and broadcasts a `StageChanged` to the room.
    #[serde(rename = "stage_join")]
    StageJoin { room_id: i64 },
    #[serde(rename = "stage_leave")]
    StageLeave { room_id: i64 },
    #[serde(rename = "stage_raise_hand")]
    StageRaiseHand { room_id: i64 },
    #[serde(rename = "stage_lower_hand")]
    StageLowerHand { room_id: i64 },
    #[serde(rename = "stage_step_down")]
    StageStepDown { room_id: i64 },
    #[serde(rename = "stage_promote")]
    StagePromote { room_id: i64, user_id: String },
    #[serde(rename = "stage_demote")]
    StageDemote { room_id: i64, user_id: String },
}

/// Recognized `kind` discriminators for a call signal. Anything else is
/// dropped before relay so a malformed client cannot inject arbitrary
/// values into a peer's call state machine.
const CALL_SIGNAL_KINDS: &[&str] = &[
    "invite", "offer", "answer", "ice", "accept", "reject", "cancel", "hangup",
];

/// Recognized `kind` discriminators for a voice-channel mesh signal.
const VOICE_SIGNAL_KINDS: &[&str] = &["offer", "answer", "ice"];

/// LC-183: recognized `kind`s for the remote-control consent handshake.
const REMOTE_CONTROL_KINDS: &[&str] = &["request", "grant", "deny", "revoke"];

/// LC-186: per-requester cap on remote-control `request` signals per minute.
const REMOTE_CONTROL_REQUEST_CAP: u32 = 10;

/// Upper bound on a relayed signaling payload. An SDP blob is a few KiB;
/// 64 KiB leaves generous headroom while bounding what one peer can push
/// through the relay in a single frame.
const MAX_CALL_PAYLOAD: usize = 64 * 1024;

/// LC-160: authorize a typed-topic subscription. `admin` requires the admin
/// role; `enclave:{id}` requires membership of that enclave; `user:{id}`
/// requires the id to be the caller's own (so a tab only subscribes to its own
/// per-user channel). Unknown / malformed topics are denied.
async fn topic_subscribe_allowed(state: &AppState, user: &User, topic: &str) -> bool {
    if topic == "admin" {
        return user.role == "admin";
    }
    match topic.split_once(':') {
        // Site admins get the same god-mode read of an enclave's member/room
        // lists over the topic that `get_landing` already grants them on the
        // page (LC-170), so a non-member admin viewing the landing still sees
        // live updates rather than a silently static list.
        Some(("enclave", id)) => match id.parse::<i64>() {
            Ok(eid) => {
                user.role == "admin"
                    || db::enclave::is_enclave_member(&state.chat, eid, &user.id)
                        .await
                        .unwrap_or(false)
            }
            Err(_) => false,
        },
        Some(("user", id)) => id == user.id,
        _ => false,
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
) -> impl IntoResponse {
    let Some(user) = user else {
        return (http::StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    ws.on_upgrade(move |socket| handle_socket(socket, state, user))
}

async fn handle_socket(socket: WebSocket, state: AppState, user: User) {
    let username = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let (conn_id, mut rx, is_first_conn) = state.hub.connect(&user.id, &username);
    if is_first_conn {
        // Re-fetch so the broadcast carries the freshest persisted status,
        // not the snapshot the inject_user middleware took at HTTP time.
        if let Ok(Some(rec)) = db::auth::find_user_by_id(&state.auth, &user.id).await {
            state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
                user_id: rec.id,
                status: rec.status,
                custom_status: rec.custom_status,
            });
        }
    }
    super::touch_user_and_maybe_broadcast(&state, &user.id).await;
    // Record that the in-app notification surface is alive for this user.
    // Bumped again, throttled, on each outbound Mentioned frame in the send
    // task below. Consumed by the email-digest "missed" predicate alongside
    // `last_active_at`; does not influence idle-flip.
    db::auth::bump_last_ws_seen(&state.auth, &user.id).await;
    let subscribed: Arc<Mutex<HashSet<i64>>> = Arc::new(Mutex::new(HashSet::new()));
    // Per-connection memory of which own-authored DM message currently shows
    // the "Seen HH:MM" caption, keyed by room_id. Used to clear the previous
    // slot when the peer reads further.
    let dm_seen_msg: Arc<Mutex<HashMap<i64, i64>>> = Arc::new(Mutex::new(HashMap::new()));
    // LC-337: the enclave this connection's page is currently viewing, learned
    // from its `enclave:{id}` SubscribeTopic frame. Every enclave page (room,
    // landing, settings) sends one on open; Home/DM pages send none. With no
    // hx-boost, each navigation is a full page load (a fresh connection), so
    // this is stable for the connection's lifetime. `render_sidebar` reads it
    // so a whole-sidebar OOB refresh renders the recipient's real context
    // instead of the DM-only shape, which would otherwise clobber the enclave
    // sidebar (blank categories + enclave rooms) on mark-all-read / mute /
    // notify-prefs / room-membership events.
    let current_enclave: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
    let (mut tx, mut rx_ws) = socket.split();

    let send_state = state.clone();
    let send_user = user.clone();
    let send_subscribed = subscribed.clone();
    let send_dm_seen = dm_seen_msg.clone();
    let send_current_enclave = current_enclave.clone();
    let send = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.tick().await;
        // Per-connection throttle for `last_ws_seen_at` bumps. The
        // connection-open bump just happened in the parent handler, so the
        // next bump is allowed in WS_BUMP_THROTTLE from now.
        const WS_BUMP_THROTTLE: Duration = Duration::from_secs(300);
        let mut last_ws_bump: std::time::Instant = std::time::Instant::now();
        loop {
            tokio::select! {
                evt = rx.recv() => {
                    match evt {
                        Ok(e) => {
                            // LC-337: snapshot this connection's current enclave
                            // (Copy) so the guard is not held across the awaits
                            // in the arms below. Read fresh each event in case
                            // the SubscribeTopic frame arrived after connect.
                            let cur_enclave = *send_current_enclave.lock().unwrap();
                            let rendered = match &e {
                                ChatEvent::NewMessage {
                                    message, client_id, ..
                                } => {
                                    render_new_message_or_bump(
                                        &send_state,
                                        message,
                                        client_id.as_deref(),
                                        &send_user,
                                        &send_subscribed,
                                    )
                                    .await
                                }
                                ChatEvent::MessageEdited { message_id, .. }
                                | ChatEvent::MessageRegrouped { message_id, .. }
                                | ChatEvent::VoiceTranscribed { message_id, .. }
                                | ChatEvent::AttachmentAltChanged { message_id, .. } => {
                                    render_edited_message(&send_state, *message_id, &send_user).await
                                }
                                ChatEvent::ReactionAdded { message_id, .. }
                                | ChatEvent::ReactionRemoved { message_id, .. } => {
                                    render_reaction_bar(&send_state, *message_id, &send_user.id).await
                                }
                                ChatEvent::Acknowledged { message_id, .. } => {
                                    render_ack_bar(&send_state, *message_id, &send_user.id).await
                                }
                                ChatEvent::AckRequiredChanged {
                                    room_id,
                                    message_id,
                                } => {
                                    render_ack_required(
                                        &send_state,
                                        &send_user,
                                        *room_id,
                                        *message_id,
                                    )
                                    .await
                                }
                                ChatEvent::PollUpdated { message_id, .. } => {
                                    render_poll(&send_state, *message_id, &send_user.id).await
                                }
                                ChatEvent::FollowUpUpdated { message_id, .. } => {
                                    render_follow_up(&send_state, *message_id, &send_user.id).await
                                }
                                ChatEvent::DmRead { user_id, room_id, last_read_message_id, read_at } => {
                                    render_dm_read(
                                        &send_state,
                                        &send_user,
                                        *room_id,
                                        user_id,
                                        *last_read_message_id,
                                        read_at,
                                        &send_dm_seen,
                                    )
                                    .await
                                }
                                ChatEvent::RoomMemberAdded { user_id, .. }
                                | ChatEvent::RoomMemberRemoved { user_id, .. } => {
                                    if user_id == &send_user.id {
                                        render_sidebar(&send_state, &send_user, cur_enclave).await
                                    } else {
                                        None
                                    }
                                }
                                // LC-161: the invitations page updates live.
                                // Both events broadcast_to_user, so this conn
                                // only receives them when send_user is the
                                // invitee; render the fresh list OOB.
                                ChatEvent::EnclaveInvitationCreated { invitee_id }
                                | ChatEvent::EnclaveInvitationResolved { invitee_id }
                                    if invitee_id == &send_user.id =>
                                {
                                    render_invitations(&send_state, &send_user).await
                                }
                                // LC-172: enclave member list changed. Broadcast
                                // on the enclave:{id} topic, so only connections
                                // subscribed to that enclave reach here. Renders
                                // the per-viewer settings members list OOB
                                // (#lc-enclave-settings-members); dropped on tabs
                                // not on that enclave's settings page.
                                ChatEvent::EnclaveMemberAdded { enclave_id, .. }
                                | ChatEvent::EnclaveMemberRemoved { enclave_id, .. }
                                | ChatEvent::EnclaveMemberRoleChanged { enclave_id, .. } => {
                                    render_enclave_members(&send_state, &send_user, *enclave_id)
                                        .await
                                }
                                // LC-174: an enclave's room set changed. Refresh
                                // the enclave-keyed sidebar nav (#sidebar-nav-{eid}),
                                // rendered per recipient; htmx applies it only on
                                // connections whose current page is that enclave.
                                ChatEvent::EnclaveRoomAdded { enclave_id, .. }
                                | ChatEvent::EnclaveRoomRemoved { enclave_id, .. } => {
                                    render_enclave_sidebar_nav(
                                        &send_state,
                                        &send_user,
                                        *enclave_id,
                                    )
                                    .await
                                }
                                // LC-173: the editor's own profile changed.
                                // broadcast_to_user fans to all their tabs;
                                // refresh the sidebar self block OOB. Gated to
                                // the editor (defensive; the event is only ever
                                // routed to them via broadcast_to_user).
                                ChatEvent::UserProfileChanged { user_id }
                                    if user_id == &send_user.id =>
                                {
                                    render_own_profile(&send_state, &send_user.id).await
                                }
                                // LC-178: the viewer's saved set changed.
                                // broadcast_to_user fans to all their tabs;
                                // refresh the /saved list OOB (dropped on tabs
                                // not on /saved). Gated to the owner (defensive).
                                ChatEvent::SavedChanged { user_id }
                                    if user_id == &send_user.id =>
                                {
                                    render_saved_list(&send_state, &send_user).await
                                }
                                // LC-250: the viewer marked everything read.
                                // broadcast_to_user fans to all their tabs;
                                // re-render the sidebar so cleared badges land
                                // live. Gated to the owner (defensive).
                                ChatEvent::ReadAllChanged { user_id }
                                    if user_id == &send_user.id =>
                                {
                                    render_sidebar(&send_state, &send_user, cur_enclave).await
                                }
                                // LC-239: the viewer saved or cleared a draft.
                                // broadcast_to_user fans to all their tabs;
                                // swap the sidebar draft badge OOB
                                // (#lc-draft-{room_id}). Tabs whose current page
                                // lacks that sidebar row drop the swap. Gated to
                                // the owner (defensive; the event is only ever
                                // routed to them via broadcast_to_user). No DB
                                // read: the event already carries the new state.
                                ChatEvent::DraftChanged {
                                    user_id,
                                    room_id,
                                    has_draft,
                                } if user_id == &send_user.id => {
                                    crate::views::ws_fragments::DraftBadgeFragment {
                                        room_id: *room_id,
                                        has_draft: *has_draft,
                                    }
                                    .render()
                                    .ok()
                                }
                                // LC-175: a user's admin-list row changed.
                                // Broadcast on the `admin` topic (only admins
                                // can subscribe), so every admin's open user
                                // list updates the row OOB. Same row for all
                                // admins; one render serves every recipient.
                                // Standalone-only: the admin routes module (and
                                // its row builder) is #[cfg(standalone)]; in
                                // saas this event is never broadcast and falls
                                // through to render_event (None).
                                #[cfg(feature = "standalone")]
                                ChatEvent::AdminUserChanged { user_id, removed } => {
                                    render_admin_user_row(&send_state, user_id, *removed).await
                                }
                                // LC-177: admin room-list row changed. Same
                                // admin-topic / standalone-only shape as the
                                // user row above.
                                #[cfg(feature = "standalone")]
                                ChatEvent::AdminRoomChanged { room_id, removed } => {
                                    render_admin_room_row(&send_state, *room_id, *removed).await
                                }
                                // LC-334: the open report set changed. Re-query +
                                // render the queue list + nav badge OOB for every
                                // admin (same fragment for all; one render serves
                                // every recipient). Standalone-only: the admin
                                // queue is #[cfg(standalone)]; in saas this event
                                // is never broadcast and falls through to
                                // render_event (None).
                                #[cfg(feature = "standalone")]
                                ChatEvent::AdminReportChanged => {
                                    render_admin_reports(&send_state).await
                                }
                                ChatEvent::ThreadReply { parent_id, message } => {
                                    render_thread_reply(
                                        &send_state,
                                        *parent_id,
                                        message,
                                        &send_user,
                                    )
                                    .await
                                }
                                ChatEvent::Mentioned {
                                    mentioned_user_id,
                                    room_id,
                                    ..
                                } if mentioned_user_id == &send_user.id => {
                                    // `MuteMode::All` suppresses the event
                                    // entirely for both room and DM kinds.
                                    // `ExceptMentions` falls through to render;
                                    // it is unreachable for DM rooms via the
                                    // API (`set_dm_mute` only writes None/All)
                                    // but a corrupt row would render rather
                                    // than crash.
                                    let allow = db::notifications::room_mute_mode(
                                        &send_state.chat,
                                        &send_user.id,
                                        *room_id,
                                    )
                                    .await
                                    .unwrap_or(db::notifications::MuteMode::None)
                                    .allows_room_mention();
                                    let notify = if allow {
                                        // The in-app surface is about to fire
                                        // for this user. Record it for the
                                        // digest, throttled per-connection so
                                        // bursty rooms do not amplify writes.
                                        if last_ws_bump.elapsed() >= WS_BUMP_THROTTLE {
                                            db::auth::bump_last_ws_seen(
                                                &send_state.auth,
                                                &send_user.id,
                                            )
                                            .await;
                                            last_ws_bump = std::time::Instant::now();
                                        }
                                        render_mentioned(&e)
                                    } else {
                                        None
                                    };
                                    // LC-179: a new mention also enters the
                                    // /activity feed, so reveal that page's
                                    // refresh bar regardless of toast-mute. The
                                    // #lc-activity-refresh id exists only on
                                    // /activity, so other connections drop it.
                                    let mut out = notify.unwrap_or_default();
                                    if let Ok(bar) =
                                        crate::views::ws_fragments::ActivityRefreshFragment.render()
                                    {
                                        out.push_str(&bar);
                                    }
                                    if out.is_empty() {
                                        None
                                    } else {
                                        Some(out)
                                    }
                                }
                                ChatEvent::MentionCleared {
                                    mentioned_user_id, ..
                                } if mentioned_user_id == &send_user.id => {
                                    render_mention_cleared(&e)
                                }
                                // LC-63: reminder toast. No mute check - the
                                // user explicitly asked to be pinged.
                                ChatEvent::Reminder { user_id, .. }
                                    if user_id == &send_user.id =>
                                {
                                    render_reminder(&e)
                                }
                                ChatEvent::RoomNotifyPrefsChanged { user_id, .. }
                                    if user_id == &send_user.id =>
                                {
                                    render_sidebar(&send_state, &send_user, cur_enclave).await
                                }
                                ChatEvent::DmMuteChanged { .. } => {
                                    // Routed only via
                                    // `Hub::broadcast_to_user(muter_id, ...)`,
                                    // so reaching this arm already implies
                                    // the recipient is the muter. Re-render
                                    // the sidebar OOB so the peer row's
                                    // greyed-link class and unread-badge
                                    // visibility flip in this tab.
                                    render_sidebar(&send_state, &send_user, cur_enclave).await
                                }
                                ChatEvent::SidebarCategoriesChanged { enclave_id } => {
                                    // LC-331: shared category state changed in
                                    // an enclave this user is a member of
                                    // (add / delete / rename / reorder / a
                                    // chat moved between categories). Re-render
                                    // the enclave-keyed sidebar nav so members
                                    // viewing that enclave pick up the change
                                    // in place. This MUST use the
                                    // `#sidebar-nav-{enclave_id}` fragment (not
                                    // the whole-sidebar DM-only shape from
                                    // `render_sidebar`): the DM-only shape has
                                    // no categories or enclave rooms, so
                                    // swapping it over `#sidebar` blanked the
                                    // chat list on add/delete and reverted a
                                    // just-completed chat move. The enclave-id
                                    // target is self-limiting, so a connection
                                    // on Home / a different enclave / a stale
                                    // subscription drops the swap.
                                    render_enclave_sidebar_nav(
                                        &send_state,
                                        &send_user,
                                        *enclave_id,
                                    )
                                    .await
                                }
                                ChatEvent::MessagePinned {
                                    room_id,
                                    message_id,
                                    ..
                                } => {
                                    // Fan out: re-render this viewer's
                                    // bubble OOB (so its hover menu flips
                                    // Pin -> Unpin) and the pinned-strip
                                    // OOB. Per-viewer because can_edit/
                                    // can_delete and the strip's "See all"
                                    // URL depend on identity.
                                    render_pin_event(
                                        &send_state,
                                        &send_user,
                                        *room_id,
                                        *message_id,
                                        true,
                                    )
                                    .await
                                }
                                ChatEvent::MessageUnpinned {
                                    room_id,
                                    message_id,
                                } => render_pin_event(
                                    &send_state,
                                    &send_user,
                                    *room_id,
                                    *message_id,
                                    false,
                                )
                                .await,
                                ChatEvent::CallSignal { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_call_signal(&e)
                                }
                                // LC-183: remote-control consent signal, gated +
                                // relayed to this user by relay_control_signal.
                                ChatEvent::RemoteControlSignal { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_control_signal(&e)
                                }
                                ChatEvent::VoiceJoined { .. }
                                | ChatEvent::VoiceLeft { .. }
                                | ChatEvent::VoiceMuteChanged { .. }
                                | ChatEvent::VoiceScreenChanged { .. } => render_voice_event(&e),
                                // LC-494: re-render the stage roster per viewer
                                // (host controls + own-state differ per viewer).
                                ChatEvent::StageChanged { room_id } => {
                                    super::stage::render_panel(&send_state, &send_user, *room_id)
                                        .await
                                }
                                ChatEvent::VoiceRoster { to_user_id, .. }
                                | ChatEvent::VoiceSignal { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_voice_event(&e)
                                }
                                // LC-393: call-transcription control + captions,
                                // per-recipient like the call/voice signals.
                                ChatEvent::TranscriptStarted { to_user_id, .. }
                                | ChatEvent::TranscriptEnded { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_transcript_control(&e)
                                }
                                ChatEvent::TranscriptSegment { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_transcript_segment(&e)
                                }
                                _ => render_event(&e),
                            };
                            if let Some(html) = rendered {
                                if tx.send(Message::Text(html.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                _ = ping.tick() => {
                    // Send an HTML-comment text frame as the heartbeat. The
                    // client uses any inbound `htmx:wsAfterMessage` to reset its
                    // half-open watchdog. Comments are not `fragment.children`
                    // for `htmx-ext-ws`, so the swap path renders a no-op.
                    if tx.send(Message::Text("<!-- ping -->".into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    while let Some(Ok(msg)) = rx_ws.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(frame) = serde_json::from_str::<ClientFrame>(text.as_str()) {
                    match frame {
                        ClientFrame::Subscribe { room_id } => {
                            let allowed = match db::chat::get_room(&state.chat, room_id).await {
                                Ok(Some(r)) if r.room_type == "dm" || r.room_type == "private" => {
                                    db::chat::is_room_member(&state.chat, room_id, &user.id)
                                        .await
                                        .unwrap_or(false)
                                }
                                Ok(Some(_)) => true,
                                _ => false,
                            };
                            if allowed {
                                state.hub.subscribe(conn_id, room_id);
                                subscribed.lock().unwrap().insert(room_id);
                            }
                        }
                        ClientFrame::SubscribeTopic { topic } => {
                            // LC-160: authorize per topic kind before joining
                            // the fan-out set. Unauthorized subscribes are
                            // silently dropped (the client simply gets no
                            // updates for a topic it cannot see).
                            if topic_subscribe_allowed(&state, &user, &topic).await {
                                state.hub.subscribe_topic(conn_id, &topic);
                                // LC-337: remember the page's enclave so a
                                // whole-sidebar OOB refresh (render_sidebar)
                                // renders this recipient's real context instead
                                // of the DM-only shape. Only `enclave:{id}`
                                // topics set it; `user:{id}` / `admin` do not.
                                if let Some(eid) = topic
                                    .strip_prefix("enclave:")
                                    .and_then(|s| s.parse::<i64>().ok())
                                {
                                    *current_enclave.lock().unwrap() = Some(eid);
                                }
                            }
                        }
                        ClientFrame::Typing { room_id } => {
                            if subscribed.lock().unwrap().contains(&room_id) {
                                state.hub.notify_typing(conn_id, room_id);
                                super::touch_user_and_maybe_broadcast(&state, &user.id).await;
                            }
                        }
                        ClientFrame::ThreadTyping { room_id, parent_id } => {
                            if subscribed.lock().unwrap().contains(&room_id) {
                                state.hub.notify_thread_typing(conn_id, room_id, parent_id);
                                super::touch_user_and_maybe_broadcast(&state, &user.id).await;
                            }
                        }
                        ClientFrame::WikiEditing { room_id } => {
                            if subscribed.lock().unwrap().contains(&room_id) {
                                state.hub.notify_wiki_editing(conn_id, room_id);
                                super::touch_user_and_maybe_broadcast(&state, &user.id).await;
                            }
                        }
                        ClientFrame::CallSignal {
                            room_id,
                            kind,
                            payload,
                        } => {
                            relay_call_signal(&state, &user, &username, room_id, &kind, payload)
                                .await;
                        }
                        ClientFrame::RemoteControlSignal { room_id, kind } => {
                            relay_control_signal(&state, &user, &username, room_id, &kind).await;
                        }
                        ClientFrame::VoiceJoin { room_id } => {
                            handle_voice_join(&state, &user, &username, conn_id, room_id).await;
                        }
                        ClientFrame::VoiceLeave { room_id } => {
                            let _ = room_id;
                            handle_voice_leave(&state, conn_id);
                        }
                        ClientFrame::VoiceSignal {
                            room_id,
                            target_user_id,
                            kind,
                            payload,
                        } => {
                            relay_voice_signal(
                                &state,
                                &user,
                                &username,
                                conn_id,
                                room_id,
                                &target_user_id,
                                &kind,
                                payload,
                            );
                        }
                        ClientFrame::VoiceMute { room_id, muted } => {
                            // Only a participant of the channel may announce mute
                            // state, and only to that channel's subscribers.
                            if state.hub.is_in_voice_room(conn_id, room_id) {
                                state.hub.broadcast_to_room(
                                    room_id,
                                    &ChatEvent::VoiceMuteChanged {
                                        room_id,
                                        user_id: user.id.clone(),
                                        muted,
                                    },
                                );
                            }
                        }
                        ClientFrame::VoiceScreen { room_id, sharing } => {
                            // Same gate as mute: only a channel participant may
                            // announce, and only to that channel's subscribers.
                            if state.hub.is_in_voice_room(conn_id, room_id) {
                                state.hub.broadcast_to_room(
                                    room_id,
                                    &ChatEvent::VoiceScreenChanged {
                                        room_id,
                                        user_id: user.id.clone(),
                                        sharing,
                                    },
                                );
                            }
                        }
                        // LC-494: stage control plane. Self-actions require room
                        // access + stage mode; promote/demote additionally
                        // require host. Every mutation broadcasts StageChanged.
                        ClientFrame::StageJoin { room_id } => {
                            handle_stage_self(&state, &user, room_id, StageAction::Join).await;
                        }
                        ClientFrame::StageLeave { room_id } => {
                            handle_stage_self(&state, &user, room_id, StageAction::Leave).await;
                        }
                        ClientFrame::StageRaiseHand { room_id } => {
                            handle_stage_self(&state, &user, room_id, StageAction::RaiseHand).await;
                        }
                        ClientFrame::StageLowerHand { room_id } => {
                            handle_stage_self(&state, &user, room_id, StageAction::LowerHand).await;
                        }
                        ClientFrame::StageStepDown { room_id } => {
                            handle_stage_self(&state, &user, room_id, StageAction::StepDown).await;
                        }
                        ClientFrame::StagePromote { room_id, user_id } => {
                            handle_stage_host(&state, &user, room_id, &user_id, true).await;
                        }
                        ClientFrame::StageDemote { room_id, user_id } => {
                            handle_stage_host(&state, &user, room_id, &user_id, false).await;
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    handle_voice_leave(&state, conn_id);
    if let Some(uid) = state.hub.disconnect(conn_id) {
        // LC-186 backstop: a hard WS drop never sends a `revoke`, so close any
        // remote-control session this user left open (call end, crash, network
        // loss). The injector's own channel-drop/heartbeat release (LC-185)
        // still handles the OS side; this only finalizes the audit row.
        let _ =
            db::remote_control_audit::end_sessions_for_user(&state.chat, &uid, "disconnect").await;
        // LC-393 backstop: a hard drop never sends /end, so finalize any
        // transcription session this user started (close + notify + post the
        // saved notice). Idempotent against a peer's explicit /end.
        super::transcripts::finalize_open_for_user(&state, &uid).await;
        // LC-494: the user's last tab closed, so drop them from any stage and
        // refresh the roster for everyone still viewing those rooms.
        for room_id in state.hub.stage_leave_all(&uid) {
            state
                .hub
                .broadcast_to_room(room_id, &ChatEvent::StageChanged { room_id });
        }
        state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
            user_id: uid,
            status: "offline".to_string(),
            custom_status: None,
        });
    }
    send.abort();
}

/// Render a single sidebar unread badge OOB swap reflecting the viewer's
/// current unread count for `room_id`. `room` is the resolved Room used to
/// pick the badge id encoding (kind="dm" + peer_id vs kind="room" + room_id).
async fn render_unread_badge(
    state: &AppState,
    viewer: &User,
    room: &models::Room,
) -> Option<String> {
    let unread = db::chat::get_unread_count(&state.chat, &viewer.id, room.id)
        .await
        .ok()?;
    if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room.id, &viewer.id)
            .await
            .ok()??;
        UnreadBadgeFragment {
            kind: "dm",
            id: &peer_id,
            unread,
            mute_mode: "none",
        }
        .render()
        .ok()
    } else {
        let id_str = room.id.to_string();
        UnreadBadgeFragment {
            kind: "room",
            id: &id_str,
            unread,
            mute_mode: "none",
        }
        .render()
        .ok()
    }
}

/// Handle a `DmRead` event. Two distinct sub-cases live in the same arm:
///
/// 1. `actor_user_id == viewer.id` - the viewer themselves opened/refreshed the
///    room in another tab. Re-render their sidebar badge so it clears live
///    across all of their sessions.
///
/// 2. `actor_user_id != viewer.id` - the peer read the viewer's messages in a
///    DM. If both parties have read receipts enabled, render the "Seen HH:MM"
///    caption under the most recent own-authored message <= peer's
///    `last_read_message_id`, and clear the previous caption slot if any.
async fn render_dm_read(
    state: &AppState,
    viewer: &User,
    room_id: i64,
    actor_user_id: &str,
    last_read_message_id: i64,
    read_at: &str,
    dm_seen_msg: &Arc<Mutex<HashMap<i64, i64>>>,
) -> Option<String> {
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;

    if actor_user_id == viewer.id {
        return render_unread_badge(state, viewer, &room).await;
    }

    if room.room_type != "dm" {
        // LC-489: a group-room read advanced a member's watermark - refresh this
        // viewer's "Seen by" bar (gating + eligibility handled in the helper).
        return render_room_seen_bar(state, viewer, room_id).await;
    }

    if !viewer.read_receipts_enabled {
        return None;
    }

    // Symmetric consent: peer must have receipts enabled for the viewer to
    // see "Seen". Look up the peer's user record from auth.
    let peer_record = db::auth::find_user_by_id(&state.auth, actor_user_id)
        .await
        .ok()??;
    if !peer_record.read_receipts_enabled {
        return None;
    }

    // Find the highest own-authored, non-deleted message in this DM with id
    // <= last_read_message_id. If none, peer hasn't read any of viewer's
    // messages yet - clear any previous caption and stop.
    let new_seen_id = db::chat::find_dm_seen_state(&state.chat, room_id, &viewer.id, actor_user_id)
        .await
        .ok()?
        .map(|(id, _)| id)
        .filter(|id| *id <= last_read_message_id);

    let prev_seen_id = {
        let map = dm_seen_msg.lock().unwrap();
        map.get(&room_id).copied()
    };

    if new_seen_id == prev_seen_id {
        return None;
    }

    let mut html = String::new();
    if let Some(prev) = prev_seen_id {
        if Some(prev) != new_seen_id {
            if let Ok(frag) = (SeenIndicatorFragment {
                message_id: prev,
                caption: None,
            })
            .render()
            {
                html.push_str(&frag);
            }
        }
    }
    if let Some(new_id) = new_seen_id {
        let hhmm = super::dm::format_hhmm(read_at);
        if let Ok(frag) = (SeenIndicatorFragment {
            message_id: new_id,
            caption: Some(&hhmm),
        })
        .render()
        {
            html.push_str(&frag);
        }
    }

    {
        let mut map = dm_seen_msg.lock().unwrap();
        match new_seen_id {
            Some(id) => {
                map.insert(room_id, id);
            }
            None => {
                map.remove(&room_id);
            }
        }
    }

    if html.is_empty() {
        None
    } else {
        Some(html)
    }
}

/// LC-489: render the group-room "Seen by" bar OOB for one viewer, or `None`
/// when the room is ineligible (DM / too large / viewer has receipts off). The
/// id-keyed `#lc-seen-{room_id}` swap drops on connections not viewing the room.
async fn render_room_seen_bar(state: &AppState, viewer: &User, room_id: i64) -> Option<String> {
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;
    let members = super::room_seen_members_if_applicable(state, viewer, &room)
        .await
        .ok()??;
    crate::views::room::RoomSeenBar {
        room_id,
        members,
        oob: true,
    }
    .render()
    .ok()
}

/// Pick the right rendering for a `NewMessage` event for this connection:
///
/// - If the viewer's connection is currently subscribed to the room (room is
///   open in the foreground), render the message into `#messages`.
/// - Else, if the message was authored by someone other than the viewer,
///   render an unread-badge bump for the sidebar.
/// - Otherwise (own message, no open subscription), render nothing.
pub async fn render_new_message_or_bump(
    state: &AppState,
    message: &models::Message,
    // LC-230: optimistic-echo dedupe id; rendered as `data-lc-client-id` on
    // the OOB wrapper, but only for the author's own connections.
    client_id: Option<&str>,
    viewer: &User,
    subscribed: &Arc<Mutex<HashSet<i64>>>,
) -> Option<String> {
    // Suppress live delivery from blocked authors. Skips both the foreground
    // render and the sidebar unread bump so a blocked user cannot poke the
    // viewer's UI.
    if db::auth::is_blocked_either_way(&state.auth, &viewer.id, &message.user_id)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let is_subscribed = subscribed.lock().unwrap().contains(&message.room_id);
    // LC-397: the author ALWAYS receives their own message echo (carrying the
    // client_id) so the optimistic placeholder is reconciled even if THIS
    // connection's room subscription has not registered yet. That gap is what
    // left enclave-room sends stuck on "Sending..." while DMs were instant: the
    // author was a broadcast recipient but, not (yet) in `subscribed`, fell to
    // the old author `return None` and got no echo. The echo is an id-keyed OOB
    // swap into #messages, so it self-limits to the tab actually viewing the
    // room; tabs elsewhere drop it harmlessly.
    if message.user_id == viewer.id {
        return render_new_message(state, message, client_id, viewer).await;
    }
    if is_subscribed {
        // The viewer (a non-author) has the room open in the foreground, so the
        // message is effectively read on arrival. Advance their last-read
        // watermark and broadcast a DmRead so the author sees a live "Seen"
        // update (in DMs) and any other tabs of this user clear their badge.
        if let Ok(read_at) =
            db::chat::set_last_read(&state.chat, &viewer.id, message.room_id, message.id).await
        {
            db::mentions::mark_mentions_read_for_room(
                &state.chat,
                &viewer.id,
                message.room_id,
                message.id,
            )
            .await
            .ok();
            let event = ChatEvent::DmRead {
                room_id: message.room_id,
                user_id: viewer.id.clone(),
                last_read_message_id: message.id,
                read_at,
            };
            state.hub.broadcast_to_room(message.room_id, &event);
        }
        return render_new_message(state, message, None, viewer).await;
    }
    // Muted rooms suppress the sidebar unread bump for background recipients.
    // The foreground branch above is intentionally left untouched: viewers
    // with the room open still see new messages and still advance their read
    // watermark.
    let mode = db::notifications::room_mute_mode(&state.chat, &viewer.id, message.room_id)
        .await
        .unwrap_or(db::notifications::MuteMode::None);
    if !mode.allows_unread_bump() {
        return None;
    }
    let room = db::chat::get_room(&state.chat, message.room_id)
        .await
        .ok()??;
    // LC-179: a new unread arrived for a background recipient. Emit the sidebar
    // unread badge AND reveal the /inbox refresh bar. The bar's #lc-inbox-refresh
    // id exists only on the /inbox page, so connections elsewhere drop it.
    let mut out = render_unread_badge(state, viewer, &room)
        .await
        .unwrap_or_default();
    if let Ok(bar) = crate::views::ws_fragments::InboxRefreshFragment.render() {
        out.push_str(&bar);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// LC-66: re-render a poll block for `user_id` (keeps per-viewer
/// `voted_by_me` highlighting correct). Returns None if the message is not
/// a poll.
async fn render_poll(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let view = crate::views::room::build_poll_view(&state.chat, &state.auth, message_id, user_id)
        .await
        .ok()??;
    crate::views::room::PollUpdateFragment { poll: &view }
        .render()
        .ok()
}

/// LC-527: re-render a follow-up checklist (`#followup-{id}`) for one recipient.
async fn render_follow_up(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let view =
        crate::views::room::build_follow_up_view(&state.chat, &state.auth, message_id, user_id)
            .await
            .ok()??;
    crate::views::room::FollowUpUpdateFragment { follow_up: &view }
        .render()
        .ok()
}

async fn render_reaction_bar(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await
        .ok()??;
    let emojis = db::custom_emojis::refs_for_room_and_user(&state.chat, m.room_id, user_id)
        .await
        .ok()
        .unwrap_or_default();
    let counts = db::chat::list_reactions(&state.chat, message_id, user_id)
        .await
        .ok()?;
    let reactor_titles = super::build_reactor_titles(state, &counts).await;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .zip(reactor_titles)
        .map(|(r, title)| ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &emojis))
        .collect();
    ReactionUpdateFragment {
        message_id,
        reactions: &reactions,
    }
    .render()
    .ok()
}

/// LC-490: re-render the ack-bar sub-region (`#ack-{id}`) for one recipient.
/// `build_ack_view` returns `None` once the requirement is cleared, which the
/// fragment renders as an empty region (so the bar disappears live).
async fn render_ack_bar(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let ack = super::build_ack_view(state, message_id, user_id)
        .await
        .ok()?;
    AckUpdateFragment { message_id, ack }.render().ok()
}

/// LC-490: re-render the whole bubble for one recipient after the ack
/// requirement is toggled, so the bar + the require/clear menu item flip.
/// Mirrors `render_pin_event` minus the pinned strip; `load_message_view_for_viewer`
/// computes the ack state per viewer internally.
async fn render_ack_required(
    state: &AppState,
    viewer: &User,
    room_id: i64,
    message_id: i64,
) -> Option<String> {
    let is_pinned = db::pinned::pinned_message_ids_for_room(&state.chat, room_id)
        .await
        .ok()?
        .contains(&message_id);
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &viewer.id, message_id)
        .await
        .unwrap_or(false);
    let view =
        super::load_message_view_for_viewer(state, viewer, message_id, is_pinned, is_bookmarked)
            .await
            .ok()?;
    crate::views::room::SingleMessageFragment {
        message: &view,
        oob: true,
    }
    .render()
    .ok()
}

/// Build a MessageView for a freshly broadcast message rendered for `viewer`.
/// Reactions are empty (a brand-new message has none); can_edit/can_delete
/// reflect viewer's role and authorship.
async fn render_new_message(
    state: &AppState,
    message: &models::Message,
    // LC-230: see render_new_message_or_bump.
    client_id: Option<&str>,
    viewer: &User,
) -> Option<String> {
    let can_edit = message.user_id == viewer.id;
    let can_delete = message.user_id == viewer.id
        || db::room_rbac::is_room_moderator(&state.chat, message.room_id, &viewer.id, &viewer.role)
            .await
            .unwrap_or(false);
    let prior = db::chat::prior_message_in_room(&state.chat, message.room_id, message.id)
        .await
        .ok()
        .flatten();
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (message.user_id.as_str(), message.created_at.as_str()),
    );
    let meta = super::resolve_msg_author(
        state,
        &message.user_id,
        message.webhook_id,
        message.email_inbox_id,
        message.bridge_id,
        message.bridge_foreign_name.as_deref(),
        message.bridge_kind.as_deref(),
        message.bridge_foreign_avatar.as_deref(),
        &viewer.id,
        message.room_id,
    )
    .await
    .ok();
    let (display_name, avatar_ext, status, custom_status, author_is_bot, actor) = match meta {
        Some(m) => (
            m.display_name,
            m.avatar_ext,
            m.status,
            m.custom_status,
            m.is_bot,
            m.actor,
        ),
        None => (
            None,
            None,
            db::auth::STATUS_ACTIVE.to_string(),
            None,
            false,
            MessageActor::User,
        ),
    };
    let attachments = db::uploads::attachments_for_message(&state.chat, message.id)
        .await
        .ok()
        .unwrap_or_default();
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[message.id])
        .await
        .ok()
        .and_then(|mut m| m.remove(&message.id))
        .unwrap_or_default();
    let custom_emojis =
        db::custom_emojis::refs_for_room_and_user(&state.chat, message.room_id, &viewer.id)
            .await
            .ok()
            .unwrap_or_default();
    // LC-323: same-enclave #channel link targets for this viewer.
    let channels = super::channel_refs_for_room(state, message.room_id, viewer)
        .await
        .unwrap_or_default();
    let quote_preview = match message.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let view = MessageView {
        id: message.id,
        room_id: message.room_id,
        user_id: message.user_id.clone(),
        username: message.author_name.clone(),
        display_name,
        avatar_ext,
        status,
        custom_status,
        created_at: message.created_at.clone(),
        edited_at: message.edited_at.clone(),
        body: message.body.clone(),
        reactions: Vec::new(),
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
        // LC-244: per-message render; the client inserts live day dividers.
        day_label: None,
        shame_enabled: false,
        shame_hidden: None,
        reply_count: 0,
        parent_id: message.parent_id,
        attachments,
        mentions,
        is_pinned: false,
        is_bookmarked: false,
        // LC-490: a brand-new message cannot yet require acknowledgement.
        ack: None,
        custom_emojis,
        channels,
        quote_preview,
        suppress_quote_preview: false,
        is_system: message.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, message.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        follow_up: crate::views::room::build_follow_up_view(
            &state.chat,
            &state.auth,
            message.id,
            &viewer.id,
        )
        .await
        .ok()
        .flatten(),
        author_is_bot,
        actor,
    };
    // LC-230: only the author's own connections carry the dedupe attribute;
    // every other viewer's frame stays byte-identical to the pre-LC-230 shape.
    let echo_client_id = if viewer.id == message.user_id {
        client_id
    } else {
        None
    };
    let mut out = render_template(&NewMessageFragment {
        message: &view,
        client_id: echo_client_id,
    })
    .ok()?;
    // LC-489: a new message resets the room's "Seen by" bar (only the author is
    // caught up to it). Append the recomputed bar OOB so it refreshes live for
    // anyone viewing the room; ineligible rooms append nothing.
    if let Some(bar) = render_room_seen_bar(state, viewer, message.room_id).await {
        out.push_str(&bar);
    }
    Some(out)
}

/// Re-fetch the edited message and render the per-viewer outerHTML OOB swap.
/// Loading from the DB ensures the broadcast picks up the canonical body and
/// edited_at timestamp rather than trusting the event payload.
async fn render_edited_message(state: &AppState, message_id: i64, viewer: &User) -> Option<String> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await
        .ok()??;
    if db::auth::is_blocked_either_way(&state.auth, &viewer.id, &m.user_id)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let meta = super::resolve_msg_author(
        state,
        &m.user_id,
        m.webhook_id,
        m.email_inbox_id,
        m.bridge_id,
        m.bridge_foreign_name.as_deref(),
        m.bridge_kind.as_deref(),
        m.bridge_foreign_avatar.as_deref(),
        &viewer.id,
        m.room_id,
    )
    .await
    .ok()?;
    let custom_emojis =
        db::custom_emojis::refs_for_room_and_user(&state.chat, m.room_id, &viewer.id)
            .await
            .ok()
            .unwrap_or_default();
    // LC-323: same-enclave #channel link targets for this viewer.
    let channels = super::channel_refs_for_room(state, m.room_id, viewer)
        .await
        .unwrap_or_default();
    let counts = db::chat::list_reactions(&state.chat, m.id, &viewer.id)
        .await
        .ok()?;
    let reactor_titles = super::build_reactor_titles(state, &counts).await;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .zip(reactor_titles)
        .map(|(r, title)| {
            ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &custom_emojis)
        })
        .collect();
    let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id)
        .await
        .ok()
        .flatten();
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (m.user_id.as_str(), m.created_at.as_str()),
    );
    let can_edit = m.user_id == viewer.id;
    let can_delete = m.user_id == viewer.id
        || db::room_rbac::is_room_moderator(&state.chat, m.room_id, &viewer.id, &viewer.role)
            .await
            .unwrap_or(false);
    let reply_count = db::chat::count_replies(&state.chat, m.id).await.ok()?;
    let parent_id = m.parent_id;
    let attachments = db::uploads::attachments_for_message(&state.chat, m.id)
        .await
        .ok()
        .unwrap_or_default();
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[m.id])
        .await
        .ok()
        .and_then(|mut x| x.remove(&m.id))
        .unwrap_or_default();
    let pinned_ids = db::pinned::pinned_message_ids_for_room(&state.chat, m.room_id)
        .await
        .ok()
        .unwrap_or_default();
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &viewer.id, m.id)
        .await
        .unwrap_or(false);
    let quote_preview = match m.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    // LC-490: recompute ack so editing a required message doesn't wipe its bar.
    let ack = super::build_ack_view(state, m.id, &viewer.id)
        .await
        .ok()
        .flatten();
    let view = MessageView {
        id: m.id,
        room_id: m.room_id,
        user_id: m.user_id,
        username: meta.username,
        display_name: meta.display_name,
        avatar_ext: meta.avatar_ext,
        status: meta.status,
        custom_status: meta.custom_status,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
        // LC-244: per-message render; the client inserts live day dividers.
        day_label: None,
        shame_enabled: false,
        shame_hidden: None,
        reply_count,
        parent_id,
        attachments,
        mentions,
        is_pinned: pinned_ids.contains(&m.id),
        is_bookmarked,
        ack,
        custom_emojis,
        channels,
        quote_preview,
        suppress_quote_preview: false,
        is_system: m.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, m.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        follow_up: crate::views::room::build_follow_up_view(
            &state.chat,
            &state.auth,
            m.id,
            &viewer.id,
        )
        .await
        .ok()
        .flatten(),
        author_is_bot: meta.is_bot,
        actor: meta.actor.clone(),
    };
    render_template(&EditedMessageFragment { message: &view }).ok()
}

/// Render a thread reply for `viewer` along with an OOB update of the
/// parent's reply-count pill in the main feed. The reply fragment targets
/// `#thread-replies-{parent_id}`, which only exists in the DOM when the
/// viewer has the thread panel open for that parent - HTMX silently drops
/// the swap otherwise. The reply-count fragment targets `#reply-count-{parent_id}`,
/// which lives under every top-level message in the main feed.
async fn render_thread_reply(
    state: &AppState,
    parent_id: i64,
    message: &models::Message,
    viewer: &User,
) -> Option<String> {
    if db::auth::is_blocked_either_way(&state.auth, &viewer.id, &message.user_id)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let meta = super::resolve_msg_author(
        state,
        &message.user_id,
        message.webhook_id,
        message.email_inbox_id,
        message.bridge_id,
        message.bridge_foreign_name.as_deref(),
        message.bridge_kind.as_deref(),
        message.bridge_foreign_avatar.as_deref(),
        &viewer.id,
        message.room_id,
    )
    .await
    .ok()?;
    let attachments = db::uploads::attachments_for_message(&state.chat, message.id)
        .await
        .ok()
        .unwrap_or_default();
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[message.id])
        .await
        .ok()
        .and_then(|mut x| x.remove(&message.id))
        .unwrap_or_default();
    let custom_emojis =
        db::custom_emojis::refs_for_room_and_user(&state.chat, message.room_id, &viewer.id)
            .await
            .ok()
            .unwrap_or_default();
    // LC-323: same-enclave #channel link targets for this viewer.
    let channels = super::channel_refs_for_room(state, message.room_id, viewer)
        .await
        .unwrap_or_default();
    let view = MessageView {
        id: message.id,
        room_id: message.room_id,
        user_id: message.user_id.clone(),
        username: meta.username,
        display_name: meta.display_name,
        avatar_ext: meta.avatar_ext,
        status: meta.status,
        custom_status: meta.custom_status,
        created_at: message.created_at.clone(),
        edited_at: message.edited_at.clone(),
        body: message.body.clone(),
        reactions: Vec::new(),
        can_edit: false,
        can_delete: false,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up: false,
        show_unread_divider: false,
        // LC-244: per-message render; the client inserts live day dividers.
        day_label: None,
        shame_enabled: false,
        shame_hidden: None,
        reply_count: 0,
        parent_id: Some(parent_id),
        attachments,
        mentions,
        is_pinned: false,
        is_bookmarked: false,
        // LC-490: thread-reply broadcast; ack bar (if any) updates via its own
        // event. A new reply isn't required at creation time.
        ack: None,
        custom_emojis,
        channels,
        quote_preview: None,
        suppress_quote_preview: false,
        is_system: message.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, message.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        follow_up: crate::views::room::build_follow_up_view(
            &state.chat,
            &state.auth,
            message.id,
            &viewer.id,
        )
        .await
        .ok()
        .flatten(),
        author_is_bot: meta.is_bot,
        actor: meta.actor.clone(),
    };
    let mut html = ThreadReplyOobFragment {
        parent_id,
        message: &view,
    }
    .render()
    .ok()?;
    let count = db::chat::count_replies(&state.chat, parent_id).await.ok()?;
    let pill = ReplyCountFragment {
        message_id: parent_id,
        room_id: message.room_id,
        reply_count: count,
        oob: true,
    }
    .render()
    .ok()?;
    html.push_str(&pill);
    Some(html)
}

/// Render a `Mentioned` event for the connected user. The hub's
/// `broadcast_to_user` already filters by id; the additional guard in the
/// caller is a belt-and-braces self-check.
fn render_mentioned(event: &ChatEvent) -> Option<String> {
    let ChatEvent::Mentioned {
        kind,
        room_id,
        room_type,
        room_label,
        message_id,
        author_label,
        snippet,
        target_path,
        ..
    } = event
    else {
        return None;
    };
    MentionedFragment {
        kind,
        room_id: *room_id,
        room_type,
        room_label,
        message_id: *message_id,
        author_label,
        snippet,
        target_path,
    }
    .render()
    .ok()
}

/// Render a `Reminder` event for the connected user (LC-63).
fn render_reminder(event: &ChatEvent) -> Option<String> {
    let ChatEvent::Reminder {
        room_label,
        snippet,
        target_path,
        ..
    } = event
    else {
        return None;
    };
    ReminderFragment {
        room_label,
        snippet,
        target_path,
    }
    .render()
    .ok()
}

fn render_mention_cleared(event: &ChatEvent) -> Option<String> {
    let ChatEvent::MentionCleared {
        room_id,
        message_id,
        ..
    } = event
    else {
        return None;
    };
    MentionClearedFragment {
        room_id: *room_id,
        message_id: *message_id,
    }
    .render()
    .ok()
}

/// Render an inbound `CallSignal` into the `#lc-call-bus` OOB fragment the
/// browser's call state machine consumes.
fn render_call_signal(event: &ChatEvent) -> Option<String> {
    let ChatEvent::CallSignal {
        room_id,
        from_user_id,
        from_name,
        kind,
        payload,
        ..
    } = event
    else {
        return None;
    };
    CallSignalFragment {
        room_id: *room_id,
        from_user_id,
        from_name,
        kind,
        payload: payload.as_deref(),
    }
    .render()
    .ok()
}

/// LC-393: render a transcription start/end control event into the
/// `#lc-transcript-bus` fragment the browser's transcribe.js drains.
fn render_transcript_control(event: &ChatEvent) -> Option<String> {
    let (kind, transcript_id, by): (&str, i64, &str) = match event {
        ChatEvent::TranscriptStarted {
            transcript_id,
            started_by_name,
            ..
        } => ("started", *transcript_id, started_by_name.as_str()),
        ChatEvent::TranscriptEnded { transcript_id, .. } => ("ended", *transcript_id, ""),
        _ => return None,
    };
    TranscriptControlFragment {
        kind,
        transcript_id,
        started_by_name: by,
    }
    .render()
    .ok()
}

/// LC-393: render one live caption line into the visible `#lc-caption-log`.
fn render_transcript_segment(event: &ChatEvent) -> Option<String> {
    let ChatEvent::TranscriptSegment {
        speaker_name, text, ..
    } = event
    else {
        return None;
    };
    TranscriptSegmentFragment { speaker_name, text }
        .render()
        .ok()
}

/// Validate and relay one WebRTC call signal to the other member of a DM
/// room. The server is a dumb relay: it confirms the sender belongs to a
/// `dm` room, that the two parties have not blocked each other, and that
/// the `kind`/`payload` are within bounds, then forwards the opaque blob.
///
/// Media kinds (`invite`/`offer`/`answer`/`ice`) go only to the peer.
/// Control kinds (`accept`/`reject`/`cancel`/`hangup`) are additionally
/// echoed to the sender's other tabs so a call answered or ended on one
/// device tears down the ringing UI on the rest.
async fn relay_call_signal(
    state: &AppState,
    user: &User,
    from_name: &str,
    room_id: i64,
    kind: &str,
    payload: Option<String>,
) {
    if !CALL_SIGNAL_KINDS.contains(&kind) {
        return;
    }
    if payload.as_ref().is_some_and(|p| p.len() > MAX_CALL_PAYLOAD) {
        return;
    }
    let room = match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) if r.room_type == "dm" => r,
        _ => return,
    };
    let members = match db::chat::list_room_member_ids(&state.chat, room_id).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if !members.iter().any(|id| id == &user.id) {
        return;
    }
    let Some(peer_id) = members.into_iter().find(|id| id != &user.id) else {
        return;
    };
    // A blocked relationship in either direction kills signaling outright.
    // `unwrap_or(true)` fails closed: a lookup error drops the signal.
    if db::auth::is_blocked_either_way(&state.auth, &user.id, &peer_id)
        .await
        .unwrap_or(true)
    {
        return;
    }
    // An `invite` claims the per-DM ringing slot. On glare (the peer is
    // already ringing us) we replay the winner's invite back to the second
    // caller so their UI flips from outgoing to incoming, and never deliver
    // the second invite to the peer. Terminating signals release the slot.
    if kind == "invite" {
        match state
            .hub
            .try_start_ringing(room_id, &user.id, from_name, payload.clone())
        {
            RingingResult::Started => {
                let event = ChatEvent::CallSignal {
                    room_id,
                    to_user_id: peer_id.clone(),
                    from_user_id: user.id.clone(),
                    from_name: from_name.to_string(),
                    kind: kind.to_string(),
                    payload: payload.clone(),
                };
                state.hub.broadcast_to_user(&peer_id, &event);
                post_call_event_message(state, user, &room, "started a call.").await;
            }
            RingingResult::DuplicateSelf => {
                // Same caller already holds the slot. Drop silently so the
                // peer is not invited a second time and a duplicate "started
                // a call" system message is not posted.
            }
            RingingResult::Glare {
                winner_id,
                from_name: winner_name,
                payload: winner_payload,
            } => {
                let event = ChatEvent::CallSignal {
                    room_id,
                    to_user_id: user.id.clone(),
                    from_user_id: winner_id,
                    from_name: winner_name,
                    kind: "invite".to_string(),
                    payload: winner_payload,
                };
                state.hub.broadcast_to_user(&user.id, &event);
            }
        }
        return;
    }

    if matches!(kind, "accept" | "reject" | "cancel" | "hangup") {
        state.hub.clear_ringing(room_id);
    }

    // LC-438: log the silent end-states as call-event rows so both members keep
    // a record (mirrors "started a call."). `user` is the signal sender: the
    // callee for a decline, the caller for a cancel/no-answer.
    match kind {
        "reject" => post_call_event_message(state, user, &room, "declined the call.").await,
        "cancel" => post_call_event_message(state, user, &room, "cancelled the call.").await,
        _ => {}
    }

    let mut recipients: Vec<String> = vec![peer_id];
    if matches!(kind, "accept" | "reject" | "cancel" | "hangup") {
        recipients.push(user.id.clone());
    }
    for to in recipients {
        let event = ChatEvent::CallSignal {
            room_id,
            to_user_id: to.clone(),
            from_user_id: user.id.clone(),
            from_name: from_name.to_string(),
            kind: kind.to_string(),
            payload: payload.clone(),
        };
        state.hub.broadcast_to_user(&to, &event);
    }
}

/// Insert a call-event system message (`body`, e.g. "started a call.",
/// "declined the call.", "cancelled the call.") into the DM `room` authored by
/// `user`, then broadcast it like any normal message so it lands in both
/// members' open conversation and bumps the sidebar for anyone not viewing the
/// room. LC-438: gives every call end-state a thread record so a decline / a
/// cancel is never silent for either party.
async fn post_call_event_message(state: &AppState, user: &User, room: &models::Room, body: &str) {
    let new_id = match db::chat::insert_system_message(&state.chat, room.id, &user.id, body).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "failed to insert call-event message");
            return;
        }
    };
    let raw = match db::chat::get_message(&state.chat, new_id).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let message = models::Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name: user.username.clone(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
        email_inbox_id: raw.email_inbox_id,
        bridge_id: raw.bridge_id,
        bridge_foreign_name: raw.bridge_foreign_name,
        bridge_kind: raw.bridge_kind,
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };
    let event = ChatEvent::NewMessage {
        message,
        is_dm: true,
        client_id: None,
    };
    if let Err(e) = super::broadcast_room_message(state, room, &event).await {
        tracing::warn!(error = %e, "failed to broadcast call-event message");
    }
}

/// LC-183: is this user "email-verified" for the purpose of the remote-control
/// gate? The two builds define verification differently (see the LC-182 design
/// doc): standalone has the `#[cfg(standalone)]` email-verification flow that
/// sets `users.email_verified_at`, so the gate is literally that column being
/// non-NULL; saas never populates that column (no verification flow), so a saas
/// account is verified by virtue of being authenticated (the platform owns
/// identity). One place to flip if saas later grows its own column check.
#[cfg(feature = "standalone")]
pub(crate) async fn remote_control_email_verified(auth: &sqlx::SqlitePool, user_id: &str) -> bool {
    db::auth::get_user_email_verified_at(auth, user_id)
        .await
        .ok()
        .flatten()
        .is_some()
}

#[cfg(not(feature = "standalone"))]
pub(crate) async fn remote_control_email_verified(
    _auth: &sqlx::SqlitePool,
    _user_id: &str,
) -> bool {
    true
}

/// LC-183: the security gate for any remote-control consent signal between
/// `a` and `b`. Both must be email-verified (per the build's definition) and
/// neither may have blocked the other. Fails closed: any lookup error denies.
pub(crate) async fn remote_control_allowed(auth: &sqlx::SqlitePool, a: &str, b: &str) -> bool {
    if db::auth::is_blocked_either_way(auth, a, b)
        .await
        .unwrap_or(true)
    {
        return false;
    }
    remote_control_email_verified(auth, a).await && remote_control_email_verified(auth, b).await
}

/// LC-183: validate + relay one remote-control consent signal to the other
/// member of a DM room. Like `relay_call_signal` it confirms the sender belongs
/// to a `dm` room and resolves the peer; additionally it enforces
/// `remote_control_allowed` (both verified, not blocked) before forwarding.
/// Consent-only - no input payload is carried here.
async fn relay_control_signal(
    state: &AppState,
    user: &User,
    from_name: &str,
    room_id: i64,
    kind: &str,
) {
    if !REMOTE_CONTROL_KINDS.contains(&kind) {
        return;
    }
    // LC-186: blunt request spam / re-request harassment. Only `request` is
    // capped (grant/deny/revoke are responses, not initiations); an over-limit
    // request drops silently, the same posture as the verified/block gate below.
    if kind == "request"
        && matches!(
            state.rate_limits.check(
                crate::rate_limit::RateLimitKind::RemoteControlRequest,
                &user.id,
                REMOTE_CONTROL_REQUEST_CAP,
            ),
            crate::rate_limit::Outcome::Deny { .. }
        )
    {
        return;
    }
    // Remote control is a DM-call feature only (mirrors relay_call_signal's
    // dm-room gate). Confirm the room is a DM; the row itself is unused.
    match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) if r.room_type == "dm" => {}
        _ => return,
    }
    let members = match db::chat::list_room_member_ids(&state.chat, room_id).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if !members.iter().any(|id| id == &user.id) {
        return;
    }
    let Some(peer_id) = members.into_iter().find(|id| id != &user.id) else {
        return;
    };
    if !remote_control_allowed(&state.auth, &user.id, &peer_id).await {
        return;
    }
    // LC-186 audit: a grant opens a session row (controller = the recipient of
    // the grant, sharer = the granter); a revoke closes the open row for the
    // room. Best-effort - an audit write failure must not block the relay.
    match kind {
        "grant" => {
            let _ =
                db::remote_control_audit::start_session(&state.chat, room_id, &peer_id, &user.id)
                    .await;
        }
        "revoke" => {
            let _ = db::remote_control_audit::end_session_by_room(&state.chat, room_id, "revoked")
                .await;
        }
        _ => {}
    }
    let event = ChatEvent::RemoteControlSignal {
        room_id,
        to_user_id: peer_id.clone(),
        from_user_id: user.id.clone(),
        from_name: from_name.to_string(),
        kind: kind.to_string(),
    };
    state.hub.broadcast_to_user(&peer_id, &event);
}

/// Render an inbound `RemoteControlSignal` into the `#lc-control-bus` OOB
/// fragment the client's consent UI consumes.
fn render_control_signal(event: &ChatEvent) -> Option<String> {
    let ChatEvent::RemoteControlSignal {
        room_id,
        from_user_id,
        from_name,
        kind,
        ..
    } = event
    else {
        return None;
    };
    crate::views::ws_fragments::ControlSignalFragment {
        room_id: *room_id,
        from_user_id,
        from_name,
        kind,
    }
    .render()
    .ok()
}

/// Render a voice-channel event into the `#lc-voice-bus` OOB fragment the
/// browser's voice mesh consumes.
fn render_voice_event(event: &ChatEvent) -> Option<String> {
    match event {
        ChatEvent::VoiceJoined {
            room_id,
            user_id,
            username,
        } => VoiceEventFragment {
            room_id: *room_id,
            kind: "joined",
            user_id,
            username,
            peers_json: "",
            payload: None,
        }
        .render()
        .ok(),
        ChatEvent::VoiceLeft { room_id, user_id } => VoiceEventFragment {
            room_id: *room_id,
            kind: "left",
            user_id,
            username: "",
            peers_json: "",
            payload: None,
        }
        .render()
        .ok(),
        ChatEvent::VoiceRoster { room_id, peers, .. } => {
            let json = serde_json::to_string(peers).ok()?;
            VoiceEventFragment {
                room_id: *room_id,
                kind: "roster",
                user_id: "",
                username: "",
                peers_json: &json,
                payload: None,
            }
            .render()
            .ok()
        }
        ChatEvent::VoiceSignal {
            room_id,
            from_user_id,
            from_name,
            kind,
            payload,
            ..
        } => VoiceEventFragment {
            room_id: *room_id,
            kind,
            user_id: from_user_id,
            username: from_name,
            peers_json: "",
            payload: payload.as_deref(),
        }
        .render()
        .ok(),
        ChatEvent::VoiceMuteChanged {
            room_id,
            user_id,
            muted,
        } => VoiceEventFragment {
            room_id: *room_id,
            kind: "mute",
            user_id,
            username: "",
            peers_json: "",
            payload: Some(if *muted { "1" } else { "0" }),
        }
        .render()
        .ok(),
        ChatEvent::VoiceScreenChanged {
            room_id,
            user_id,
            sharing,
        } => VoiceEventFragment {
            room_id: *room_id,
            kind: "screen",
            user_id,
            username: "",
            peers_json: "",
            payload: Some(if *sharing { "1" } else { "0" }),
        }
        .render()
        .ok(),
        _ => None,
    }
}

/// Handle a `voice_join` frame: validate the room is an accessible voice
/// channel, register the connection in the hub, hand the joiner the current
/// roster, and announce the joiner to everyone already in the channel.
async fn handle_voice_join(
    state: &AppState,
    user: &User,
    username: &str,
    conn_id: ConnId,
    room_id: i64,
) {
    // LC-493: the mesh serves both enclave voice channels (`is_voice`) and
    // ad-hoc huddles attached to a group text room. DMs keep their dedicated
    // 1:1 call path, so they are excluded here.
    match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) if r.room_type != "dm" => {}
        _ => return,
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin)
        .await
        .unwrap_or(false)
    {
        return;
    }
    let peers = state.hub.voice_join(conn_id, room_id);
    // The joiner gets the roster so it can open a peer connection to each.
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::VoiceRoster {
            room_id,
            to_user_id: user.id.clone(),
            peers,
        },
    );
    // Existing voice members learn about the joiner and await its offer (the
    // newest joiner is always the mesh offerer). The event also fans out to
    // anyone just *viewing* the voice page so their participants preview
    // stays in sync without waiting for a page reload.
    let joined = ChatEvent::VoiceJoined {
        room_id,
        user_id: user.id.clone(),
        username: username.to_string(),
    };
    state.hub.broadcast_to_room(room_id, &joined);
}

/// Remove `conn_id` from its voice channel (if any) and tell the remaining
/// participants. Idempotent - safe to call on both explicit leave and
/// disconnect.
fn handle_voice_leave(state: &AppState, conn_id: ConnId) {
    let Some((room_id, user_id, _)) = state.hub.voice_leave(conn_id) else {
        return;
    };
    // Tell every subscriber of the room, not just remaining voice members: a
    // viewer who is *not* in the call still needs the update so their
    // participants preview stays accurate.
    let left = ChatEvent::VoiceLeft { room_id, user_id };
    state.hub.broadcast_to_room(room_id, &left);
    // LC-393 Phase 2: if that was the last participant, finalize any open
    // transcription session for the channel (save + post the notice). Spawned
    // because this fn is sync; AppState is cheap to clone (Arc-backed).
    if state.hub.voice_room_users(room_id).is_empty() {
        let st = state.clone();
        tokio::spawn(async move {
            super::transcripts::finalize_open_for_room(&st, room_id).await;
        });
    }
}

/// LC-494: stage self-actions (the actor operates on their own membership).
enum StageAction {
    Join,
    Leave,
    RaiseHand,
    LowerHand,
    StepDown,
}

/// Apply a stage self-action for `user` in `room_id`, then broadcast the new
/// roster. Gated on room access + stage mode (a DM or stage-off room no-ops).
async fn handle_stage_self(state: &AppState, user: &User, room_id: i64, action: StageAction) {
    if !super::stage::stage_enabled(state, room_id).await {
        return;
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin)
        .await
        .unwrap_or(false)
    {
        return;
    }
    match action {
        StageAction::Join => state.hub.stage_join(room_id, &user.id),
        StageAction::Leave => state.hub.stage_leave(room_id, &user.id),
        StageAction::RaiseHand => {
            state.hub.stage_raise_hand(room_id, &user.id);
        }
        StageAction::LowerHand => state.hub.stage_lower_hand(room_id, &user.id),
        StageAction::StepDown => state.hub.stage_demote(room_id, &user.id),
    }
    state
        .hub
        .broadcast_to_room(room_id, &ChatEvent::StageChanged { room_id });
}

/// LC-494: host grants (`promote`) or revokes (`!promote`) the floor for
/// `target` in `room_id`. Requires the actor be a stage host.
async fn handle_stage_host(
    state: &AppState,
    user: &User,
    room_id: i64,
    target: &str,
    promote: bool,
) {
    if !super::stage::stage_enabled(state, room_id).await {
        return;
    }
    if !super::stage::is_host(state, user, room_id).await {
        return;
    }
    if promote {
        state.hub.stage_promote(room_id, target);
    } else {
        state.hub.stage_demote(room_id, target);
    }
    state
        .hub
        .broadcast_to_room(room_id, &ChatEvent::StageChanged { room_id });
}

/// Relay one mesh signal (offer/answer/ice) to a specific peer in the same
/// voice channel. The server validates channel co-membership and bounds the
/// payload; it never interprets the SDP/ICE blob.
#[allow(clippy::too_many_arguments)]
fn relay_voice_signal(
    state: &AppState,
    user: &User,
    from_name: &str,
    conn_id: ConnId,
    room_id: i64,
    target_user_id: &str,
    kind: &str,
    payload: Option<String>,
) {
    if !VOICE_SIGNAL_KINDS.contains(&kind) {
        return;
    }
    if payload.as_ref().is_some_and(|p| p.len() > MAX_CALL_PAYLOAD) {
        return;
    }
    if !state.hub.is_in_voice_room(conn_id, room_id) {
        return;
    }
    if !state
        .hub
        .voice_room_users(room_id)
        .iter()
        .any(|u| u == target_user_id)
    {
        return;
    }
    state.hub.broadcast_to_user(
        target_user_id,
        &ChatEvent::VoiceSignal {
            room_id,
            to_user_id: target_user_id.to_string(),
            from_user_id: user.id.clone(),
            from_name: from_name.to_string(),
            kind: kind.to_string(),
            payload,
        },
    );
}

/// Render a full sidebar OOB replacement for `viewer` reflecting current
/// room/DM membership and unread counts. Used to live-update the sidebar
/// when membership changes (new DM, room kick, room invite).
/// LC-161: re-render the viewer's pending-invitations region as an OOB
/// fragment. Pushed when an EnclaveInvitation{Created,Resolved} event reaches
/// this connection (broadcast_to_user already targets the right user); the
/// swap is a no-op on tabs not currently showing the invitations page.
async fn render_invitations(state: &AppState, viewer: &User) -> Option<String> {
    let invs = db::enclave::list_invitations_for_user(&state.chat, &viewer.id)
        .await
        .ok()?;
    crate::views::enclave::InvitationsLiveFragment { invitations: &invs }
        .render()
        .ok()
}

/// LC-170: re-render the enclave landing-page member list as an OOB fragment.
/// The list is read-only (label + role), so this does not depend on the
/// recipient's identity; one fragment is correct for every subscriber.
/// LC-170/172: re-render the enclave member list for both surfaces that show
/// it. The landing-page list (`#lc-enclave-members`) is read-only, so one
/// render serves every subscriber; the settings-page list
/// (`#lc-enclave-settings-members`) carries kick/role/transfer controls gated
/// on the recipient's `can_delete`, so it is rendered per `viewer`. Both
/// fragments are emitted in one frame: a connection on the landing page matches
/// only the first id, a connection on settings matches only the second, and
/// htmx silently drops the unmatched OOB target.
/// LC-174: re-render the enclave-keyed sidebar nav for `viewer` after a room is
/// added to / removed from `enclave_id`. Per recipient so unread / mention /
/// active state is correct; the fragment's `#sidebar-nav-{enclave_id}` target
/// is present only on a connection currently viewing that enclave, so htmx
/// drops it for everyone else (Home, a different enclave, a stale subscriber).
async fn render_enclave_sidebar_nav(
    state: &AppState,
    viewer: &User,
    enclave_id: i64,
) -> Option<String> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_sidebar(state, viewer, Some(enclave_id))
        .await
        .ok()?;
    crate::views::ws_fragments::SidebarNavLiveFragment {
        user: viewer,
        enclave_id,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
    }
    .render()
    .ok()
}

/// LC-175: render the OOB swap for a changed admin user-list row. `removed`
/// emits an `hx-swap-oob="delete"` tombstone for `#user-{id}`; otherwise the
/// fresh row (oob) is rendered. Returns None if a non-removal event races a
/// just-deleted user (the row is gone). Standalone-only: the admin routes
/// module that builds the row is `#[cfg(standalone)]`.
#[cfg(feature = "standalone")]
async fn render_admin_user_row(state: &AppState, user_id: &str, removed: bool) -> Option<String> {
    if removed {
        return crate::views::admin::UserRowDeleteFragment {
            user_id: user_id.to_string(),
        }
        .render()
        .ok();
    }
    let view = super::admin::build_admin_user_view(state, user_id)
        .await
        .ok()??;
    crate::views::admin::UserRowFragment {
        u: &view,
        oob: true,
    }
    .render()
    .ok()
}

/// LC-177: render the OOB swap for a changed admin room-list row. `removed`
/// emits an `hx-swap-oob="delete"` tombstone for `#room-{id}`; otherwise the
/// fresh row (oob) is rendered. Returns None if a non-removal event races a
/// just-archived room. Standalone-only (admin routes module is
/// `#[cfg(standalone)]`).
#[cfg(feature = "standalone")]
async fn render_admin_room_row(state: &AppState, room_id: i64, removed: bool) -> Option<String> {
    if removed {
        return crate::views::admin::RoomRowDeleteFragment { room_id }
            .render()
            .ok();
    }
    let view = super::admin::build_admin_room_view(state, room_id)
        .await
        .ok()??;
    crate::views::admin::RoomRowFragment {
        r: &view,
        oob: true,
    }
    .render()
    .ok()
}

/// LC-334: render the report-queue OOB fragment (`#admin-reports-list` + nav
/// badge) for the `admin` topic after the open-report set changes. Same fragment
/// for every admin, so one render serves all recipients. Standalone-only (the
/// admin queue is `#[cfg(standalone)]`).
#[cfg(feature = "standalone")]
async fn render_admin_reports(state: &AppState) -> Option<String> {
    let reports = super::report::build_report_views(state).await.ok()?;
    let open_count = reports.len() as i64;
    crate::views::report::AdminReportsOob {
        reports,
        open_count,
    }
    .render()
    .ok()
}

/// LC-173: re-render the sidebar self block (avatar + name + custom status) as
/// an OOB fragment after the user edits their profile. Re-fetches the user so
/// the fresh display_name / avatar_ext / custom_status are reflected. Returns
/// None if the user row vanished (deleted account mid-flight).
async fn render_own_profile(state: &AppState, user_id: &str) -> Option<String> {
    let record = db::auth::find_user_by_id(&state.auth, user_id)
        .await
        .ok()??;
    let user = User::from(record);
    crate::views::ws_fragments::OwnProfileLiveFragment { user: &user }
        .render()
        .ok()
}

/// LC-178: re-render the viewer's /saved list as an OOB fragment after a
/// bookmark/unbookmark. Reuses the page's row-builder so the live list matches
/// a fresh page load.
async fn render_saved_list(state: &AppState, viewer: &User) -> Option<String> {
    let entries = super::bookmarks::build_saved_rows(state, viewer)
        .await
        .ok()?;
    crate::views::bookmarks::SavedListFragment { entries: &entries }
        .render()
        .ok()
}

/// LC-172: re-render the enclave settings members list as an OOB fragment for
/// `viewer`. Per-viewer because the role-toggle / kick / transfer controls are
/// gated on `can_delete`. Dropped on tabs not on that enclave's settings page
/// (the `#lc-enclave-settings-members` id is absent there).
async fn render_enclave_members(
    state: &AppState,
    viewer: &User,
    enclave_id: i64,
) -> Option<String> {
    let enclave = db::enclave::get_enclave(&state.chat, enclave_id)
        .await
        .ok()??;
    let members = db::enclave::list_members(&state.chat, enclave_id)
        .await
        .ok()?;
    let member_views = super::enclave::resolve_member_views(state, members)
        .await
        .ok()?;
    let membership = db::enclave::get_membership(&state.chat, enclave_id, &viewer.id)
        .await
        .ok()
        .flatten();
    let can_delete = crate::perms::enclave_can_delete(membership.map(|m| m.role), &viewer.role);
    crate::views::enclave::EnclaveSettingsMembersLiveFragment {
        enclave: &enclave,
        members: &member_views,
        can_delete,
    }
    .render()
    .ok()
}

async fn render_sidebar(
    state: &AppState,
    viewer: &User,
    current_enclave: Option<i64>,
) -> Option<String> {
    // LC-337: render the recipient's CURRENT context, not a hardcoded Home /
    // DM-only shape. The OOB target `#sidebar` exists on every page, so a
    // None-enclave render would clobber an enclave viewer's sidebar (blank
    // categories + enclave rooms) and rename the nav id from
    // `sidebar-nav-{eid}` to `sidebar-nav`. `current_enclave` comes from the
    // connection's `enclave:{id}` topic subscription (see handle_socket).
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_sidebar(state, viewer, current_enclave)
        .await
        .ok()?;
    let switcher = super::load_switcher(state, viewer, sidebar_current_enclave)
        .await
        .ok()?;
    SidebarUpdateFragment {
        user: viewer,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
    }
    .render()
    .ok()
}

/// Build the OOB fragments for a MessagePinned / MessageUnpinned event:
/// the re-rendered message bubble (OOB so its hover menu flips for this
/// viewer) followed by the pinned strip (also OOB). Returns the two
/// fragments concatenated in one Text frame. The bubble OOB swap is a
/// no-op for subscribers who don't currently have `#msg-{id}` in their
/// DOM (e.g. the message scrolled off, or this viewer is on a different
/// page) - htmx silently drops unmatched OOB targets. If the message has
/// been deleted between the mutation and the broadcast, the bubble part
/// is skipped and only the strip is emitted.
async fn render_pin_event(
    state: &AppState,
    viewer: &User,
    room_id: i64,
    message_id: i64,
    is_pinned: bool,
) -> Option<String> {
    use askama::Template;
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;
    let pin_path = if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room_id, &viewer.id)
            .await
            .ok()??;
        format!("/dm/{peer_id}/pins")
    } else {
        format!("/room/{room_id}/pins")
    };
    let strip_html = super::pinned::build_strip_fragment(state, room_id, pin_path, true)
        .await
        .ok()?
        .render()
        .ok()?;
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &viewer.id, message_id)
        .await
        .unwrap_or(false);
    let bubble_html = match super::load_message_view_for_viewer(
        state,
        viewer,
        message_id,
        is_pinned,
        is_bookmarked,
    )
    .await
    {
        Ok(view) => crate::views::room::SingleMessageFragment {
            message: &view,
            oob: true,
        }
        .render()
        .ok()
        .unwrap_or_default(),
        Err(_) => String::new(),
    };
    Some(format!("{bubble_html}{strip_html}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn auth_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations/auth")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    async fn verify(pool: &SqlitePool, id: &str) {
        sqlx::query("UPDATE users SET email_verified_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    // LC-183: the remote-control gate allows two email-verified, mutually
    // unblocked users. (In saas builds verification is implicit, so the
    // verify() calls are no-ops there but the assertion still holds.)
    #[tokio::test]
    async fn gate_allows_two_verified_unblocked_users() {
        let auth = auth_pool().await;
        let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
        let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
        verify(&auth, &a).await;
        verify(&auth, &b).await;
        assert!(remote_control_allowed(&auth, &a, &b).await);
    }

    // Standalone-only: an unverified peer is denied in either direction. In
    // saas the column is not the gate, so this assertion does not apply.
    #[cfg(feature = "standalone")]
    #[tokio::test]
    async fn gate_denies_when_a_peer_is_unverified() {
        let auth = auth_pool().await;
        let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
        let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
        verify(&auth, &a).await; // b stays unverified
        assert!(!remote_control_allowed(&auth, &a, &b).await);
        assert!(!remote_control_allowed(&auth, &b, &a).await);
    }

    // A block in either direction denies, regardless of verification/build.
    #[tokio::test]
    async fn gate_denies_when_blocked_either_way() {
        let auth = auth_pool().await;
        let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
        let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
        verify(&auth, &a).await;
        verify(&auth, &b).await;
        db::auth::block_user(&auth, &a, &b).await.unwrap();
        assert!(!remote_control_allowed(&auth, &a, &b).await);
        assert!(!remote_control_allowed(&auth, &b, &a).await);
    }
}
