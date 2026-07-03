#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom};
use crate::views::room::MessageView;
use crate::ws::events::ChatEvent;

#[derive(Template)]
#[template(path = "ws/new_message.html")]
pub struct NewMessageFragment<'a> {
    pub message: &'a MessageView,
    /// LC-230: optimistic-echo dedupe id, rendered as `data-lc-client-id` on
    /// the OOB wrapper. `Some(...)` only on the author's own connections; the
    /// attribute lives on the wrapper (discarded by the beforeend swap), so
    /// it never reaches the persistent DOM - live.js reads it from the
    /// incoming fragment in `htmx:oobBeforeSwap` to remove the placeholder.
    pub client_id: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "ws/edited_message.html")]
pub struct EditedMessageFragment<'a> {
    pub message: &'a MessageView,
}

#[derive(Template)]
#[template(path = "ws/deleted_message.html")]
pub struct DeletedMessageFragment {
    pub message_id: i64,
}

/// OOB fragment that removes a message node from the DOM outright. Used
/// by the retention sweep (`ChatEvent::MessagePurged`). Distinct from
/// `DeletedMessageFragment`, which renders a "[deleted]" tombstone for
/// moderation soft-delete; retention erases the message visually as well
/// as in the DB so no metadata-leak placeholder remains.
#[derive(Template)]
#[template(path = "ws/purged_message.html")]
pub struct PurgedMessageFragment {
    pub message_id: i64,
}

#[derive(Template)]
#[template(path = "ws/typing.html")]
pub struct TypingFragment<'a> {
    pub username: &'a str,
}

#[derive(Template)]
#[template(path = "ws/stopped_typing.html")]
pub struct StoppedTypingFragment;

/// LC-498: "X is editing the wiki" presence banner (mirrors the typing
/// fragment). Recipient-independent, so like typing it renders one shared
/// string for all viewers.
#[derive(Template)]
#[template(path = "ws/wiki_editing.html")]
pub struct WikiEditingFragment<'a> {
    pub username: &'a str,
}

#[derive(Template)]
#[template(path = "ws/wiki_stopped_editing.html")]
pub struct WikiStoppedEditingFragment;

/// LC-179: OOB reveal of the /inbox "new unread - refresh" bar
/// (`#lc-inbox-refresh`). Dropped on connections not on /inbox.
#[derive(Template)]
#[template(path = "ws/inbox_refresh.html")]
pub struct InboxRefreshFragment;

/// LC-179: OOB reveal of the /activity "new activity - refresh" bar
/// (`#lc-activity-refresh`). Dropped on connections not on /activity.
#[derive(Template)]
#[template(path = "ws/activity_refresh.html")]
pub struct ActivityRefreshFragment;

#[derive(Template)]
#[template(path = "ws/reaction_update.html")]
pub struct ReactionUpdateFragment<'a> {
    pub message_id: i64,
    pub reactions: &'a [super::room::ReactionView],
}

/// LC-490: out-of-band swap of the ack bar (`#ack-{message_id}`) for one
/// recipient after an acknowledge. `ack: None` renders an empty region (the
/// requirement was cleared); `Some(_)` re-renders the roster + the viewer's
/// Acknowledge / acknowledged state.
#[derive(Template)]
#[template(path = "ws/ack_update.html")]
pub struct AckUpdateFragment {
    pub message_id: i64,
    pub ack: Option<super::room::AckState>,
}

/// Out-of-band swap that updates a sidebar unread badge. The badge id is
/// `unread-{kind}-{id}` where kind is "room" or "dm" and id is the room_id
/// (for rooms) or peer user_id (for DMs, from the badge owner's perspective).
/// `unread = 0` clears the badge; positive values render a count chip.
#[derive(Template)]
#[template(path = "ws/unread_badge.html")]
pub struct UnreadBadgeFragment<'a> {
    pub kind: &'a str,
    pub id: &'a str,
    pub unread: i64,
    /// "none" | "except_mentions" | "all". The WS badge is rendered for
    /// background recipients in the chat path, where the mute filter in
    /// `render_new_message_or_bump` has already short-circuited muted rooms.
    /// Pass `"none"` here so the template logic stays unified with the
    /// page-render include site.
    pub mute_mode: &'a str,
}

/// LC-239: OOB swap of one sidebar conversation's draft indicator
/// (`#lc-draft-{room_id}`) when the viewer saves or clears a draft.
/// Rendered per recipient in the WS send task, gated to the draft owner
/// (the event is `broadcast_to_user`), so `render_event` returns `None`.
#[derive(Template)]
#[template(path = "ws/draft_badge.html")]
pub struct DraftBadgeFragment {
    pub room_id: i64,
    pub has_draft: bool,
}

/// Out-of-band swap that updates the "Seen HH:MM" caption under one DM
/// message. `caption = None` clears the slot; `Some(text)` populates it. The
/// element id is `seen-{message_id}`, present in the DOM for every
/// own-authored DM message via `room/message.html`.
#[derive(Template)]
#[template(path = "ws/seen_indicator.html")]
pub struct SeenIndicatorFragment<'a> {
    pub message_id: i64,
    pub caption: Option<&'a str>,
}

/// OOB sidebar replacement. Used when the user's room/DM membership changes
/// so the new entry shows up live without a refresh. Renders the entire
/// sidebar partial wrapped to swap on the existing #sidebar element.
#[derive(Template)]
#[template(path = "ws/sidebar_update.html")]
pub struct SidebarUpdateFragment<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
}

/// LC-173: OOB swap of the sidebar self block (`#sidebar-self`) when the user
/// edits their own profile, so the avatar / display name / custom status
/// refresh in every one of their tabs. Shares `partials/sidebar_self.html`
/// with the sidebar so the page and the live update render identically.
#[derive(Template)]
#[template(path = "ws/own_profile_live.html")]
pub struct OwnProfileLiveFragment<'a> {
    pub user: &'a User,
}

/// LC-174: OOB swap of the enclave-keyed sidebar nav (`#sidebar-nav-{enclave_id}`)
/// when a room is added to / removed from that enclave, so members currently
/// viewing it (its landing, settings, or one of its rooms) see the room list
/// change without a reload. Rendered per recipient (unread / mention / active
/// state) via the same fields the full sidebar uses; `enclave_id` drives the
/// target id and `sidebar_current_enclave` must equal `Some(enclave_id)`.
#[derive(Template)]
#[template(path = "ws/sidebar_nav_live.html")]
pub struct SidebarNavLiveFragment<'a> {
    pub user: &'a User,
    pub enclave_id: i64,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
}

/// OOB-append a single thread reply into `#thread-replies-{parent_id}`. The
/// container only exists in the DOM when the recipient has the thread panel
/// open for that parent, so HTMX silently drops the swap otherwise.
#[derive(Template)]
#[template(path = "ws/thread_reply.html")]
pub struct ThreadReplyOobFragment<'a> {
    pub parent_id: i64,
    pub message: &'a super::room::MessageView,
}

#[derive(Template)]
#[template(path = "ws/thread_typing.html")]
pub struct ThreadTypingFragment<'a> {
    pub parent_id: i64,
    pub username: &'a str,
}

#[derive(Template)]
#[template(path = "ws/thread_stopped_typing.html")]
pub struct ThreadStoppedTypingFragment {
    pub parent_id: i64,
}

/// OOB-appended notification trigger. The client's `#lc-notify-bus`
/// MutationObserver consumes this element to fire a title flash, favicon
/// dot, optional sound, and (when the tab is hidden) a browser
/// Notification.
#[derive(Template)]
#[template(path = "ws/mentioned.html")]
pub struct MentionedFragment<'a> {
    pub kind: &'a str,
    pub room_id: i64,
    pub room_type: &'a str,
    pub room_label: &'a str,
    pub message_id: i64,
    pub author_label: &'a str,
    pub snippet: &'a str,
    pub target_path: &'a str,
}

/// LC-63: OOB-appended reminder toast. Same `#lc-notify-bus` mechanism as
/// `MentionedFragment` but tagged `data-event="reminder"` so the client
/// fires a banner/notification without bumping unread-mention counts.
#[derive(Template)]
#[template(path = "ws/reminder.html")]
pub struct ReminderFragment<'a> {
    pub room_label: &'a str,
    pub snippet: &'a str,
    pub target_path: &'a str,
}

/// OOB-appended decrement signal: an earlier mention was retracted (the
/// message was edited to remove the @-token, or deleted). The client
/// decrements its in-memory unread-mention count for the room.
#[derive(Template)]
#[template(path = "ws/mention_cleared.html")]
pub struct MentionClearedFragment {
    pub room_id: i64,
    pub message_id: i64,
}

/// Tiny inline script that updates every avatar status dot for a user across
/// the rendered DOM (sidebar entries, message rows, DM headers). Uses a
/// CSS-attribute selector instead of OOB-by-id because the same user's dot
/// appears many times per page and HTMX OOB swaps only the first id match.
/// Fields are pre-encoded as JSON literals to defend against quote/script
/// injection in the user-controlled custom status text.
#[derive(Template)]
#[template(path = "ws/user_status_update.html")]
pub struct UserStatusUpdateFragment {
    pub user_id_json: String,
    pub status_json: String,
    pub custom_status_json: String,
}

/// OOB-appended WebRTC call signal. The client's `#lc-call-bus`
/// MutationObserver consumes this element and feeds the `kind` + `payload`
/// into the per-DM `RTCPeerConnection` state machine. `payload` carries an
/// opaque SDP/ICE blob for media kinds and is absent for the pure-control
/// kinds (invite/accept/reject/cancel/hangup).
#[derive(Template)]
#[template(path = "ws/call_signal.html")]
pub struct CallSignalFragment<'a> {
    pub room_id: i64,
    pub from_user_id: &'a str,
    pub from_name: &'a str,
    pub kind: &'a str,
    pub payload: Option<&'a str>,
}

/// LC-183: OOB-appended remote-control consent signal. The client's
/// `#lc-control-bus` MutationObserver consumes the `kind`
/// (request/grant/deny/revoke) to drive the consent UI. No payload - the input
/// stream is a separate data channel (LC-184).
#[derive(Template)]
#[template(path = "ws/control_signal.html")]
pub struct ControlSignalFragment<'a> {
    pub room_id: i64,
    pub from_user_id: &'a str,
    pub from_name: &'a str,
    pub kind: &'a str,
}

/// OOB-appended voice-channel event. The client's `#lc-voice-bus`
/// MutationObserver consumes this and feeds it into the per-channel mesh
/// of `RTCPeerConnection`s. `kind` is `joined` / `left` / `roster` /
/// `offer` / `answer` / `ice`; the other fields carry whichever payload
/// that kind needs (empty string when not applicable).
#[derive(Template)]
#[template(path = "ws/voice_event.html")]
pub struct VoiceEventFragment<'a> {
    pub room_id: i64,
    pub kind: &'a str,
    pub user_id: &'a str,
    pub username: &'a str,
    pub peers_json: &'a str,
    pub payload: Option<&'a str>,
}

/// LC-393: OOB-appended transcription control event (started / ended). The
/// client's `#lc-transcript-bus` MutationObserver consumes `kind` to show or
/// hide the consent banner + caption panel and to start/stop its local mic
/// capture, posting segments to `transcript_id`.
#[derive(Template)]
#[template(path = "ws/transcript_control.html")]
pub struct TranscriptControlFragment<'a> {
    pub kind: &'a str,
    pub transcript_id: i64,
    pub started_by_name: &'a str,
}

/// LC-393: one live caption line, OOB-appended straight into the visible
/// `#lc-caption-log` panel (no client JS needed to display it).
#[derive(Template)]
#[template(path = "ws/transcript_segment.html")]
pub struct TranscriptSegmentFragment<'a> {
    pub speaker_name: &'a str,
    pub text: &'a str,
}

/// Render a ChatEvent as an HTML fragment with hx-swap-oob attributes for
/// events that do not depend on the recipient. Per-recipient events
/// (NewMessage, MessageEdited, ReactionAdded/Removed, DmRead,
/// RoomMemberAdded/Removed) are rendered directly in the WS handler where
/// the recipient identity is available.
pub fn render_event(event: &ChatEvent) -> Option<String> {
    match event {
        ChatEvent::MessageDeleted { message_id, .. } => DeletedMessageFragment {
            message_id: *message_id,
        }
        .render()
        .ok(),
        ChatEvent::MessagePurged { message_id, .. } => PurgedMessageFragment {
            message_id: *message_id,
        }
        .render()
        .ok(),
        ChatEvent::UserTyping { username, .. } => TypingFragment { username }.render().ok(),
        ChatEvent::UserStoppedTyping { .. } => StoppedTypingFragment.render().ok(),
        ChatEvent::WikiEditing { username, .. } => {
            WikiEditingFragment { username }.render().ok()
        }
        ChatEvent::WikiStoppedEditing { .. } => WikiStoppedEditingFragment.render().ok(),
        ChatEvent::ThreadTyping {
            parent_id,
            username,
            ..
        } => ThreadTypingFragment {
            parent_id: *parent_id,
            username,
        }
        .render()
        .ok(),
        ChatEvent::ThreadStoppedTyping { parent_id, .. } => ThreadStoppedTypingFragment {
            parent_id: *parent_id,
        }
        .render()
        .ok(),
        ChatEvent::NewMessage { .. }
        | ChatEvent::MessageEdited { .. }
        | ChatEvent::MessageRegrouped { .. }
        // LC-483: rendered per recipient in the WS send task (re-render message).
        | ChatEvent::VoiceTranscribed { .. }
        | ChatEvent::ReactionAdded { .. }
        | ChatEvent::ReactionRemoved { .. }
        | ChatEvent::AckRequiredChanged { .. }
        | ChatEvent::Acknowledged { .. }
        | ChatEvent::RoomMemberAdded { .. }
        | ChatEvent::RoomMemberRemoved { .. }
        | ChatEvent::DmRead { .. }
        | ChatEvent::UserMuted { .. }
        | ChatEvent::UserBanned { .. }
        | ChatEvent::UserKicked { .. }
        | ChatEvent::ThreadReply { .. }
        | ChatEvent::EnclaveMemberAdded { .. }
        | ChatEvent::EnclaveMemberRemoved { .. }
        | ChatEvent::EnclaveMemberRoleChanged { .. }
        | ChatEvent::EnclaveRoomAdded { .. }
        | ChatEvent::EnclaveRoomRemoved { .. }
        | ChatEvent::SidebarCategoriesChanged { .. }
        | ChatEvent::EnclaveInvitationCreated { .. }
        | ChatEvent::EnclaveInvitationResolved { .. }
        // LC-175/177: rendered per recipient in the WS send task (admin row OOB).
        | ChatEvent::AdminUserChanged { .. }
        | ChatEvent::AdminRoomChanged { .. }
        // LC-334: rendered per recipient in the WS send task (report queue OOB);
        // in saas the event never fires, so the None here is the saas behavior.
        | ChatEvent::AdminReportChanged
        // LC-178: rendered per recipient in the WS send task (/saved list OOB).
        | ChatEvent::SavedChanged { .. }
        // LC-239: rendered per recipient in the WS send task (sidebar draft
        // badge OOB), so the recipient-independent path emits nothing.
        | ChatEvent::DraftChanged { .. }
        // LC-250: rendered per recipient in the WS send task (sidebar OOB).
        | ChatEvent::ReadAllChanged { .. }
        | ChatEvent::Mentioned { .. }
        | ChatEvent::Reminder { .. }
        | ChatEvent::PollUpdated { .. }
        | ChatEvent::FollowUpUpdated { .. }
        | ChatEvent::MentionCleared { .. }
        | ChatEvent::RoomNotifyPrefsChanged { .. }
        | ChatEvent::DmMuteChanged { .. }
        | ChatEvent::MessagePinned { .. }
        | ChatEvent::MessageUnpinned { .. }
        | ChatEvent::CallSignal { .. }
        // LC-183: rendered per recipient in the WS send task (#lc-control-bus).
        | ChatEvent::RemoteControlSignal { .. }
        | ChatEvent::VoiceJoined { .. }
        | ChatEvent::VoiceLeft { .. }
        | ChatEvent::VoiceRoster { .. }
        | ChatEvent::VoiceSignal { .. }
        | ChatEvent::VoiceMuteChanged { .. }
        | ChatEvent::VoiceScreenChanged { .. }
        // LC-494: stage roster re-rendered per recipient in the WS send task.
        | ChatEvent::StageChanged { .. }
        // LC-393: transcription control + captions are rendered per recipient
        // in the WS send task (to_user_id targeting), like the call signals.
        | ChatEvent::TranscriptStarted { .. }
        | ChatEvent::TranscriptSegment { .. }
        | ChatEvent::TranscriptEnded { .. }
        // LC-173: rendered per recipient in the WS send task (own sidebar
        // self block), so the recipient-independent path emits nothing.
        | ChatEvent::UserProfileChanged { .. } => None,
        ChatEvent::UserStatusChanged {
            user_id,
            status,
            custom_status,
        } => UserStatusUpdateFragment {
            user_id_json: serde_json::to_string(user_id).ok()?,
            status_json: serde_json::to_string(status).ok()?,
            custom_status_json: serde_json::to_string(custom_status).ok()?,
        }
        .render()
        .ok(),
    }
}
