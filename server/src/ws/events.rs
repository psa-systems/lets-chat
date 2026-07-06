use serde::{Deserialize, Serialize};

use crate::models::Message;

/// Events sent from server to client over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    NewMessage {
        message: Message,
        is_dm: bool,
        /// LC-230: opaque client-generated id echoed from the composer's
        /// `MessageForm.client_id`. The author's own WS render carries it as
        /// a `data-lc-client-id` attribute on the OOB wrapper so the sending
        /// tab can remove its optimistic placeholder when the canonical
        /// render arrives. Always `None` for non-composer senders (webhooks,
        /// email ingress, bridges, scheduled sends, slash/poll output,
        /// quarantine release, call-started notices).
        #[serde(default)]
        client_id: Option<String>,
    },
    MessageEdited {
        message_id: i64,
        room_id: i64,
        new_body: String,
        edited_at: String,
    },
    /// Emitted by the delete handler when removing a header message exposes a
    /// follow-up that should be promoted to a header. Recipients re-render the
    /// referenced message with the current grouping flag.
    MessageRegrouped {
        message_id: i64,
        room_id: i64,
    },
    /// LC-483: a voice message's server-side transcription finished and was
    /// stored. Recipients re-render the referenced message so the transcript
    /// appears under the waveform player (same per-viewer render path as
    /// `MessageEdited`/`MessageRegrouped`).
    VoiceTranscribed {
        message_id: i64,
        room_id: i64,
    },
    /// LC-537: an image's alt text was added/edited; re-render the whole
    /// message for every viewer (the alt lives inside the attachment fragment).
    AttachmentAltChanged {
        message_id: i64,
        room_id: i64,
    },
    MessageDeleted {
        message_id: i64,
        room_id: i64,
    },
    /// Hard-delete by the retention sweep. Distinct from `MessageDeleted`
    /// (which is the moderation soft-delete tombstone): retention is true
    /// erasure, and the OOB fragment removes the message node from the
    /// DOM entirely with `hx-swap-oob="delete"`. No "[deleted]" placeholder
    /// because a visible tombstone would leak the metadata that a message
    /// existed there, defeating the compliance posture retention is for.
    MessagePurged {
        message_id: i64,
        room_id: i64,
    },
    UserTyping {
        room_id: i64,
        user_id: String,
        username: String,
    },
    UserStoppedTyping {
        room_id: i64,
        user_id: String,
    },
    /// LC-498: someone is actively editing this room's wiki (presence only -
    /// true co-editing is deferred). Broadcast to the room except the editor so
    /// other viewers of the room About page see an "X is editing the wiki"
    /// banner. Transient, no DB; mirrors `UserTyping`.
    WikiEditing {
        room_id: i64,
        user_id: String,
        username: String,
    },
    /// LC-498: the wiki editor went idle / blurred / saved; clears the banner.
    WikiStoppedEditing {
        room_id: i64,
        user_id: String,
    },
    RoomMemberAdded {
        room_id: i64,
        user_id: String,
    },
    DmRead {
        room_id: i64,
        user_id: String,
        last_read_message_id: i64,
        read_at: String,
    },
    RoomMemberRemoved {
        room_id: i64,
        user_id: String,
    },
    UserMuted {
        user_id: String,
        muted_until: Option<String>,
    },
    UserBanned {
        user_id: String,
    },
    UserKicked {
        user_id: String,
        room_id: i64,
    },
    ReactionAdded {
        message_id: i64,
        room_id: i64,
        emoji: String,
        user_id: String,
    },
    ReactionRemoved {
        message_id: i64,
        room_id: i64,
        emoji: String,
        user_id: String,
    },
    /// LC-490: a message's acknowledgement requirement was toggled on or off.
    /// Re-renders the full bubble per viewer so the ack bar and the
    /// require/clear menu item flip.
    AckRequiredChanged {
        room_id: i64,
        message_id: i64,
    },
    /// LC-490: someone acknowledged a message. Re-renders the ack-bar
    /// sub-region (`#ack-{id}`) per viewer so the roster + own state stay correct.
    Acknowledged {
        room_id: i64,
        message_id: i64,
    },
    /// LC-66: a poll's tallies changed (a vote, or the poll closed). The
    /// per-connection render re-renders the poll block for the viewer so
    /// `voted_by_me` highlighting stays correct across clients.
    PollUpdated {
        message_id: i64,
        room_id: i64,
    },
    /// LC-527: a follow-up checklist changed (an item toggled or (un)claimed).
    /// Re-rendered per-viewer so `assigned_to_me` highlighting stays correct.
    FollowUpUpdated {
        message_id: i64,
        room_id: i64,
    },
    EnclaveMemberAdded {
        enclave_id: i64,
        user_id: String,
    },
    EnclaveMemberRemoved {
        enclave_id: i64,
        user_id: String,
    },
    /// LC-170: a member's enclave role changed (member <-> admin). Drives the
    /// live member-list refresh on the `enclave:{id}` topic; no sidebar effect.
    EnclaveMemberRoleChanged {
        enclave_id: i64,
        user_id: String,
    },
    EnclaveRoomAdded {
        enclave_id: i64,
        room_id: i64,
    },
    /// Shared room-category state changed (create / rename / delete /
    /// assign / reorder). Every member of `enclave_id` re-renders their
    /// sidebar so live tabs catch up.
    SidebarCategoriesChanged {
        enclave_id: i64,
    },
    EnclaveRoomRemoved {
        enclave_id: i64,
        room_id: i64,
    },
    EnclaveInvitationCreated {
        invitee_id: String,
    },
    EnclaveInvitationResolved {
        invitee_id: String,
    },
    UserStatusChanged {
        user_id: String,
        status: String,
        custom_status: Option<String>,
    },
    /// LC-173: a user edited their own profile (avatar / display name / bio).
    /// Routed via `broadcast_to_user(user_id, ...)` so every tab of the editor
    /// refreshes their sidebar self block (avatar + name) without a reload.
    /// Cross-surface refresh of OTHER viewers' author rows is a separate
    /// follow-up.
    UserProfileChanged {
        user_id: String,
    },
    /// LC-178: the user's saved-message set changed (bookmark / unbookmark).
    /// Routed via `broadcast_to_user(user_id, ...)` so every tab refreshes the
    /// /saved list OOB without a reload. Tabs not on /saved drop the swap.
    SavedChanged {
        user_id: String,
    },
    /// LC-250: the viewer used "mark all as read", clearing unread + mention
    /// state across their conversations. Routed via
    /// `broadcast_to_user(user_id, ...)` so every tab re-renders the sidebar
    /// (badges cleared) without a reload.
    ReadAllChanged {
        user_id: String,
    },
    /// LC-239: the viewer's draft for `room_id` was saved (`has_draft = true`)
    /// via the debounced composer PUT, or cleared (`has_draft = false`) by a
    /// send / schedule / empty-body delete. Routed via
    /// `broadcast_to_user(user_id, ...)` so every tab updates the sidebar
    /// draft indicator (`#lc-draft-{room_id}`) OOB without a reload. Tabs
    /// whose current page lacks that sidebar row drop the swap silently.
    DraftChanged {
        user_id: String,
        room_id: i64,
        has_draft: bool,
    },
    /// LC-175: a user's admin-list row changed (ban / mute / role / quota) or
    /// was removed (delete). Broadcast on the `admin` topic so every admin's
    /// open user list refreshes the `#user-{id}` row OOB. `removed` swaps the
    /// row out with `hx-swap-oob="delete"`.
    AdminUserChanged {
        user_id: String,
        removed: bool,
    },
    /// LC-177: a room's admin-list row changed (edit / invite / regenerate) or
    /// was removed (archive). Broadcast on the `admin` topic so every admin's
    /// open room list refreshes the `#room-{id}` row OOB. `removed` swaps the
    /// row out with `hx-swap-oob="delete"`.
    AdminRoomChanged {
        room_id: i64,
        removed: bool,
    },
    /// LC-334: the open message-report set changed (a report was filed, resolved,
    /// or dismissed). Broadcast on the `admin` topic; the WS send task re-queries
    /// and renders the `#admin-reports-list` + `#admin-reports-badge` OOB
    /// fragment for each admin. Standalone-only on the render side (the admin
    /// queue is `#[cfg(standalone)]`); in saas it falls through to `render_event`
    /// (None).
    AdminReportChanged,
    /// New thread reply rooted at `parent_id`. Recipients with the panel open
    /// for that parent append the reply; everyone updates the parent's
    /// "N replies" pill.
    ThreadReply {
        parent_id: i64,
        message: Message,
    },
    /// Typing indicator scoped to the thread panel.
    ThreadTyping {
        room_id: i64,
        parent_id: i64,
        user_id: String,
        username: String,
    },
    ThreadStoppedTyping {
        room_id: i64,
        parent_id: i64,
        user_id: String,
    },
    /// A user was @-mentioned in a room message, or a DM was sent to them.
    /// Routed via `Hub::broadcast_to_user(mentioned_user_id, ...)`.
    Mentioned {
        /// "mention" for a real `@username` ping in a room; "dm" for an
        /// implicit DM ping. The client uses this to label the notification.
        kind: String,
        room_id: i64,
        /// "public" | "private" | "dm" - lets the client format the
        /// notification title without an extra DB lookup.
        room_type: String,
        /// Display label for the room (e.g. "#general") or DM peer name.
        room_label: String,
        message_id: i64,
        mentioned_user_id: String,
        author_label: String,
        /// First ~140 chars of the body, plain-text. Mention chips and links
        /// are stripped to keep the notification readable.
        snippet: String,
        /// "/room/{id}" or "/dm/{peer_id}" - target path for the click handler.
        target_path: String,
    },
    /// A previously-fired mention is no longer current (the message was
    /// edited to remove the @-token, or the message was deleted). The client
    /// decrements its in-memory unread-mention count for that room.
    MentionCleared {
        room_id: i64,
        mentioned_user_id: String,
        message_id: i64,
    },
    /// LC-63: a message reminder fired for its owner. Routed via
    /// `Hub::broadcast_to_user(user_id, ...)` and dispatched to Web Push.
    /// Unlike `Mentioned`, it does not bump the client's unread-mention
    /// counts (the user asked to be pinged; it is not a new message), and
    /// it ignores room mute (an explicit reminder overrides muting).
    Reminder {
        user_id: String,
        room_id: i64,
        /// "public" | "private" | "dm" - lets the client label the toast.
        room_type: String,
        /// "#general" or the DM peer label.
        room_label: String,
        message_id: i64,
        /// Plain-text preview of the reminded message.
        snippet: String,
        /// Deep link to the message, e.g. "/room/{id}#msg-{mid}".
        target_path: String,
    },
    /// Per-user notification of a notify-prefs change. Recipients re-render
    /// their sidebar OOB so the muted-room class flips and badges hide/show
    /// across all of their open tabs. Routed via
    /// `Hub::broadcast_to_user(user_id, ...)`.
    RoomNotifyPrefsChanged {
        user_id: String,
        room_id: i64,
        mute_mode: String,
    },
    /// Per-user notification of a DM-mute toggle. Recipients re-render their
    /// sidebar OOB so the muted-peer class flips and badges hide/show across
    /// all of their open tabs. Routed via
    /// `Hub::broadcast_to_user(user_id, ...)`.
    DmMuteChanged {
        /// The room id of the affected DM (the canonical key for the
        /// (muter, dm_room_id) row in `room_notification_settings`).
        dm_room_id: i64,
        /// The peer of the affected DM. Carried for clarity; the sidebar
        /// re-render does not consult it directly.
        peer_user_id: String,
        muted: bool,
    },
    /// A message was pinned in a room (or DM, which is just a room with
    /// `room_type = 'dm'`). Routed via `Hub::broadcast_to_room(room_id, ...)`
    /// so every subscriber to the room re-renders their pinned-strip.
    MessagePinned {
        room_id: i64,
        message_id: i64,
        pinned_by: String,
    },
    /// The inverse of `MessagePinned`. Recipients re-render the strip; the
    /// strip render filters by `messages.deleted_at IS NULL` so a soft-
    /// deleted message that was pinned naturally drops out without a
    /// dedicated event.
    MessageUnpinned {
        room_id: i64,
        message_id: i64,
    },
    /// WebRTC 1:1 call signaling relayed verbatim between the two members of
    /// a DM room. The server never interprets `payload`; it is an opaque
    /// blob (an SDP offer/answer or an ICE candidate, JSON-encoded by the
    /// browser) produced and consumed by the peers' `RTCPeerConnection`.
    /// `kind` is one of: `invite`, `offer`, `answer`, `ice`, `accept`,
    /// `reject`, `cancel`, `hangup`. Routed via
    /// `Hub::broadcast_to_user(to_user_id, ...)`.
    CallSignal {
        room_id: i64,
        /// The intended recipient. The WS send task renders the frame only
        /// for the connection whose user matches this id (belt-and-braces
        /// on top of `broadcast_to_user`'s own targeting).
        to_user_id: String,
        from_user_id: String,
        /// Display name of the signal's sender, shown in the callee's
        /// incoming-call UI without an extra DB lookup.
        from_name: String,
        kind: String,
        payload: Option<String>,
    },
    /// LC-183: remote-control consent handshake between the two members of a
    /// DM call. `kind` is `request` / `grant` / `deny` / `revoke`. Relayed only
    /// after a server-side gate (both peers email-verified, neither blocked).
    /// Carries no input payload - the actual input stream rides a separate
    /// WebRTC data channel (LC-184); this is consent signaling only. Routed via
    /// `Hub::broadcast_to_user(to_user_id, ...)`.
    RemoteControlSignal {
        room_id: i64,
        to_user_id: String,
        from_user_id: String,
        /// Display name of the sender, for the consent prompt without a lookup.
        from_name: String,
        kind: String,
    },
    /// A user joined an enclave voice channel. Broadcast to everyone already
    /// in that channel so they render the new participant and await the
    /// joiner's offer (the newest joiner is always the mesh offerer).
    VoiceJoined {
        room_id: i64,
        user_id: String,
        username: String,
    },
    /// A user left a voice channel (explicit leave or disconnect). Broadcast
    /// to the remaining participants so they tear down that peer connection.
    VoiceLeft {
        room_id: i64,
        user_id: String,
    },
    /// LC-494: the stage control-plane roster changed (join/leave, hand
    /// raised/lowered, promote/demote). Broadcast to the room; each client
    /// re-renders the stage panel for itself (host controls are per-viewer).
    /// Carries no payload - recipients read the current roster from the hub.
    StageChanged {
        room_id: i64,
    },
    /// Sent only to a freshly-joined participant: the users already in the
    /// channel. The joiner opens a peer connection to each and sends the
    /// offer. `peers` is a list of `(user_id, username)` pairs.
    VoiceRoster {
        room_id: i64,
        /// Restricts rendering to the joiner; mirrors `CallSignal::to_user_id`.
        to_user_id: String,
        peers: Vec<(String, String)>,
    },
    /// Mesh signaling (offer/answer/ice) relayed verbatim between two members
    /// of the same voice channel. `payload` is opaque (SDP or ICE JSON).
    VoiceSignal {
        room_id: i64,
        to_user_id: String,
        from_user_id: String,
        from_name: String,
        kind: String,
        payload: Option<String>,
    },
    /// LC-402: a voice participant toggled their microphone. Broadcast to the
    /// whole room (like `VoiceJoined`) so every other client shows the mute
    /// indicator on that participant's tile. Active-speaker state is detected
    /// locally from each peer's received audio, so only this discrete mute flag
    /// needs propagating over the wire.
    VoiceMuteChanged {
        room_id: i64,
        user_id: String,
        muted: bool,
    },
    /// LC-408: a voice participant started or stopped sharing their screen.
    /// Broadcast to the room so every client pins that participant's tile to
    /// the stage (and drops it back to the even grid when sharing ends). The
    /// screen video already rides the participant's normal video track; this is
    /// only the discrete "is this tile a screen share" flag the receiver cannot
    /// otherwise infer.
    VoiceScreenChanged {
        room_id: i64,
        user_id: String,
        sharing: bool,
    },
    /// LC-393: call transcription turned ON for a DM call. Broadcast to each
    /// member (mirrors `CallSignal`'s per-recipient `to_user_id` targeting) so
    /// both see the consent banner and their client starts capturing their own
    /// mic. `transcript_id` is the open session the clients post segments to.
    TranscriptStarted {
        room_id: i64,
        to_user_id: String,
        transcript_id: i64,
        started_by_name: String,
    },
    /// LC-393: one finalized speech result, broadcast to each member as a live
    /// caption line. `speaker_id`/`speaker_name` attribute it (the speaker is
    /// whoever's browser produced it - each transcribes only its own mic).
    TranscriptSegment {
        room_id: i64,
        to_user_id: String,
        transcript_id: i64,
        speaker_id: String,
        speaker_name: String,
        text: String,
    },
    /// LC-393: transcription turned OFF (hangup / toggle / disconnect backstop).
    /// Clears the banner + stops the client's local capture.
    TranscriptEnded {
        room_id: i64,
        to_user_id: String,
        transcript_id: i64,
    },
}

/// Control frames sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientControl {
    Subscribe { room_id: i64 },
    Unsubscribe { room_id: i64 },
    Typing { room_id: i64 },
    ThreadTyping { room_id: i64, parent_id: i64 },
}
