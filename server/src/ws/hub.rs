use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::ws::events::ChatEvent;

/// One pending DM call. Held in `Hub::ringing` keyed by DM `room_id` so a
/// glare-resolution decision is identical for every connection of both peers,
/// regardless of which page each one is on. Cleared on accept/reject/cancel/
/// hangup or by the 60s timeout task spawned alongside the slot.
#[derive(Debug, Clone)]
pub struct RingingSlot {
    pub caller_id: String,
    pub from_name: String,
    /// The invite payload (`"audio"` or `"video"`) - replayed verbatim if a
    /// glare loser has to be re-delivered the winner's invite.
    pub payload: Option<String>,
    pub started: Instant,
}

/// Outcome of `Hub::try_start_ringing`. Lets the caller decide whether to
/// relay the new invite, drop it, or replay the existing winner's invite
/// back to the second caller (the glare loser).
pub enum RingingResult {
    /// Slot was vacant and we now own it.
    Started,
    /// Same caller already has the slot - drop the duplicate.
    DuplicateSelf,
    /// The peer is already calling us. The new invite must NOT be relayed;
    /// the existing winner's invite should be replayed to the second caller
    /// so their UI flips from outgoing to incoming.
    Glare {
        winner_id: String,
        from_name: String,
        payload: Option<String>,
    },
}

/// How long a ringing slot is allowed to live before the server gives up and
/// clears it. Caps damage when a client dies mid-call without sending the
/// terminating signal that would normally release the slot.
const RINGING_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Unique identifier for a WebSocket connection.
pub type ConnId = u64;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_conn_id() -> ConnId {
    NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed)
}

/// A connected user's handle.
struct Connection {
    user_id: String,
    username: String,
    tx: broadcast::Sender<ChatEvent>,
}

/// Global hub managing room subscriptions and event fan-out.
pub struct Hub {
    /// conn_id -> Connection
    connections: DashMap<ConnId, Connection>,
    /// room_id -> set of conn_ids subscribed
    rooms: DashMap<i64, HashSet<ConnId>>,
    /// LC-160: typed topic string ("enclave:5", "user:abc", "admin") -> set of
    /// conn_ids subscribed. Parallel to `rooms` so room fan-out is unchanged;
    /// this carries the non-room live-update channels. Subscription is
    /// authorized per topic kind at subscribe time (see ws.rs).
    topics: DashMap<String, HashSet<ConnId>>,
    /// user_id -> set of conn_ids (a user may have multiple tabs)
    user_conns: DashMap<String, HashSet<ConnId>>,
    /// (room_id, user_id) -> last typing Instant (ephemeral, no DB)
    typing: DashMap<(i64, String), std::time::Instant>,
    /// (room_id, parent_id, user_id) -> last typing Instant for thread typing.
    /// Kept separate from `typing` so room-composer typing and thread-composer
    /// typing don't bleed into each other's UI surfaces.
    thread_typing: DashMap<(i64, i64, String), std::time::Instant>,
    /// LC-498: (room_id, user_id) -> last "editing the wiki" Instant. Ephemeral
    /// presence like `typing` (no DB); drives the "X is editing the wiki" banner
    /// on the room About page. This is the presence-only slice of live wiki
    /// co-editing - true concurrent editing (CRDT/OT) is deferred.
    wiki_editing: DashMap<(i64, String), std::time::Instant>,
    /// room_id -> conn_ids currently joined to that enclave voice channel.
    /// Ephemeral presence, like `typing` - never persisted to the DB.
    voice_rooms: DashMap<i64, HashSet<ConnId>>,
    /// conn_id -> the voice channel it joined, for O(1) cleanup on leave or
    /// disconnect. A connection is in at most one voice channel.
    voice_conn: DashMap<ConnId, i64>,
    /// DM `room_id` -> the single pending 1:1 call. Holds the caller's id so
    /// a second simultaneous invite for the same DM (the classic SIP "glare"
    /// race) is resolved server-side: the first invite wins, the second is
    /// converted into an incoming UI for its sender by replaying the winner.
    ringing: DashMap<i64, RingingSlot>,
    /// LC-494: ephemeral "stage" control-plane state per room (speaker/listener
    /// roles + raised hands). Keyed by `room_id`; participants are keyed by
    /// user_id (a person is on stage regardless of how many tabs they have).
    /// No media here - the audio transport lands in the LC-512 SFU follow-up.
    stages: DashMap<i64, StageState>,
    /// LC-853: room_id -> user_ids currently announcing a live screen share in
    /// that huddle/voice channel. Ephemeral like `voice_rooms` - fed by the
    /// `voice_screen` frame, cleared on the matching `sharing: false`, on
    /// voice-leave, and on disconnect. The huddle remote-control relay uses it
    /// to route a control request to the sharer, so it is authoritative enough
    /// to fail closed on: an empty or ambiguous set refuses the request.
    voice_screens: DashMap<i64, HashSet<String>>,
    /// LC-853: room_id -> the single pending remote-control request for that
    /// huddle (requester waiting on the sharer's Allow/Deny). One slot per
    /// room, never a queue: a second request while one is pending is refused
    /// as busy. Entries expire after [`CONTROL_PENDING_TTL`] so an abandoned
    /// prompt (sharer closed the tab mid-prompt) cannot wedge the room.
    control_pending: DashMap<i64, ControlPending>,
}

/// LC-853: one pending huddle control request (see `Hub::control_pending`).
struct ControlPending {
    requester_id: String,
    at: std::time::Instant,
}

/// LC-853: how long a pending control request stays claimable. Longer than the
/// client's ~12s request timeout and ~30s prompt auto-deny, so the server slot
/// outlives every legitimate answer window but still self-clears.
const CONTROL_PENDING_TTL: std::time::Duration = std::time::Duration::from_secs(45);

/// LC-494: the live roster of one stage room. `participants` is everyone on the
/// stage (speakers + listeners); `speakers` is the subset granted the floor;
/// `hands` is listeners who requested to speak.
#[derive(Default, Clone)]
pub struct StageState {
    pub participants: HashSet<String>,
    pub speakers: HashSet<String>,
    pub hands: HashSet<String>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            rooms: DashMap::new(),
            topics: DashMap::new(),
            user_conns: DashMap::new(),
            typing: DashMap::new(),
            thread_typing: DashMap::new(),
            wiki_editing: DashMap::new(),
            voice_rooms: DashMap::new(),
            voice_conn: DashMap::new(),
            ringing: DashMap::new(),
            stages: DashMap::new(),
            voice_screens: DashMap::new(),
            control_pending: DashMap::new(),
        }
    }

    /// Try to claim the per-DM ringing slot for `room_id` on behalf of
    /// `caller_id`. Returns one of:
    ///
    /// - `Started`: slot was vacant; the caller now owns it. The caller
    ///   should relay the invite to the peer.
    /// - `DuplicateSelf`: the same user already holds the slot (a retry, or
    ///   a second tab inviting at the same time). Drop the new invite.
    /// - `Glare { winner_id, .. }`: the peer is already ringing us; the new
    ///   invite must not be relayed. Replay the stored invite (the fields
    ///   returned here) back to the new caller so its UI flips to incoming.
    pub fn try_start_ringing(
        self: &Arc<Self>,
        room_id: i64,
        caller_id: &str,
        from_name: &str,
        payload: Option<String>,
    ) -> RingingResult {
        use dashmap::mapref::entry::Entry;
        match self.ringing.entry(room_id) {
            Entry::Occupied(slot) => {
                let existing = slot.get();
                // A stale slot whose lifetime has passed gets evicted in place
                // so a fresh invite from either party can claim it. Mirrors
                // what the spawned timeout task would have done shortly.
                if existing.started.elapsed() >= RINGING_TTL {
                    slot.remove();
                    let new_slot = RingingSlot {
                        caller_id: caller_id.to_string(),
                        from_name: from_name.to_string(),
                        payload,
                        started: Instant::now(),
                    };
                    self.ringing.insert(room_id, new_slot);
                    self.spawn_ringing_ttl(room_id);
                    return RingingResult::Started;
                }
                if existing.caller_id == caller_id {
                    return RingingResult::DuplicateSelf;
                }
                RingingResult::Glare {
                    winner_id: existing.caller_id.clone(),
                    from_name: existing.from_name.clone(),
                    payload: existing.payload.clone(),
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(RingingSlot {
                    caller_id: caller_id.to_string(),
                    from_name: from_name.to_string(),
                    payload,
                    started: Instant::now(),
                });
                self.spawn_ringing_ttl(room_id);
                RingingResult::Started
            }
        }
    }

    /// Release the ringing slot for `room_id`. Called when either party
    /// emits a terminating signal (`accept`/`reject`/`cancel`/`hangup`) so
    /// the next invite for that DM can claim a fresh slot immediately.
    pub fn clear_ringing(&self, room_id: i64) {
        self.ringing.remove(&room_id);
    }

    /// Spawn a one-shot task that evicts the ringing slot for `room_id`
    /// after `RINGING_TTL`, but only if the slot still matches the moment
    /// this task was spawned (so a quick clear+reclaim does not orphan a
    /// kill-switch from an earlier call).
    fn spawn_ringing_ttl(self: &Arc<Self>, room_id: i64) {
        let hub = self.clone();
        let started_at = self
            .ringing
            .get(&room_id)
            .map(|s| s.started)
            .unwrap_or_else(Instant::now);
        tokio::spawn(async move {
            tokio::time::sleep(RINGING_TTL).await;
            if let Some(slot) = hub.ringing.get(&room_id) {
                if slot.started == started_at {
                    drop(slot);
                    hub.ringing.remove(&room_id);
                }
            }
        });
    }

    /// Join `conn_id` to voice channel `room_id`. Returns the
    /// `(user_id, username)` of every connection already in the channel so
    /// the joiner can open a peer connection to each. A connection may be in
    /// at most one voice channel; any prior membership is dropped silently.
    pub fn voice_join(&self, conn_id: ConnId, room_id: i64) -> Vec<(String, String)> {
        if let Some((_, old)) = self.voice_conn.remove(&conn_id) {
            if let Some(mut set) = self.voice_rooms.get_mut(&old) {
                set.remove(&conn_id);
            }
        }
        let existing: Vec<(String, String)> = self
            .voice_rooms
            .get(&room_id)
            .map(|set| {
                set.iter()
                    .filter_map(|c| {
                        self.connections
                            .get(c)
                            .map(|conn| (conn.user_id.clone(), conn.username.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.voice_rooms.entry(room_id).or_default().insert(conn_id);
        self.voice_conn.insert(conn_id, room_id);
        existing
    }

    /// Remove `conn_id` from whatever voice channel it was in. Returns
    /// `(room_id, user_id, username)` when it was in one so the caller can
    /// broadcast a `VoiceLeft`. Must be called while the connection still
    /// exists in `connections` - i.e. before `disconnect`.
    pub fn voice_leave(&self, conn_id: ConnId) -> Option<(i64, String, String)> {
        let (_, room_id) = self.voice_conn.remove(&conn_id)?;
        if let Some(mut set) = self.voice_rooms.get_mut(&room_id) {
            set.remove(&conn_id);
        }
        self.voice_rooms.retain(|_, set| !set.is_empty());
        let conn = self.connections.get(&conn_id)?;
        Some((room_id, conn.user_id.clone(), conn.username.clone()))
    }

    /// The deduped user_ids currently in voice channel `room_id`.
    pub fn voice_room_users(&self, room_id: i64) -> Vec<String> {
        let mut seen = HashSet::new();
        self.voice_rooms
            .get(&room_id)
            .map(|set| {
                set.iter()
                    .filter_map(|c| self.connections.get(c).map(|conn| conn.user_id.clone()))
                    .filter(|uid| seen.insert(uid.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// True when `conn_id` is currently joined to voice channel `room_id`.
    pub fn is_in_voice_room(&self, conn_id: ConnId, room_id: i64) -> bool {
        self.voice_conn
            .get(&conn_id)
            .map(|r| *r == room_id)
            .unwrap_or(false)
    }

    // ---- LC-853 huddle remote-control state ---------------------------

    /// Record (or clear) `user_id`'s screen-share announcement for `room_id`.
    pub fn set_voice_screen(&self, room_id: i64, user_id: &str, sharing: bool) {
        if sharing {
            self.voice_screens
                .entry(room_id)
                .or_default()
                .insert(user_id.to_string());
        } else {
            if let Some(mut set) = self.voice_screens.get_mut(&room_id) {
                set.remove(user_id);
            }
            self.voice_screens.retain(|_, set| !set.is_empty());
        }
    }

    /// The user_ids currently announcing a screen share in `room_id`.
    pub fn voice_screen_sharers(&self, room_id: i64) -> Vec<String> {
        self.voice_screens
            .get(&room_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Claim the room's pending-request slot for `requester_id`. Returns false
    /// when an unexpired request already holds it (one at a time, no queue).
    pub fn set_control_pending(&self, room_id: i64, requester_id: &str) -> bool {
        if self.control_pending_active(room_id) {
            return false;
        }
        self.control_pending.insert(
            room_id,
            ControlPending {
                requester_id: requester_id.to_string(),
                at: std::time::Instant::now(),
            },
        );
        true
    }

    /// True when an unexpired control request is pending for `room_id`.
    /// An expired entry is dropped on the way (lazy expiry - nothing polls).
    pub fn control_pending_active(&self, room_id: i64) -> bool {
        let expired = match self.control_pending.get(&room_id) {
            Some(p) => p.at.elapsed() > CONTROL_PENDING_TTL,
            None => return false,
        };
        if expired {
            self.control_pending.remove(&room_id);
            return false;
        }
        true
    }

    /// Consume the room's pending request, returning the requester it belonged
    /// to. `None` when nothing (unexpired) is pending - a grant/deny with no
    /// live request has nobody to answer and must be dropped.
    pub fn take_control_pending(&self, room_id: i64) -> Option<String> {
        if !self.control_pending_active(room_id) {
            return None;
        }
        self.control_pending
            .remove(&room_id)
            .map(|(_, p)| p.requester_id)
    }

    /// Drop the pending request if `user_id` owns it (requester left the
    /// huddle or disconnected mid-prompt).
    pub fn clear_control_pending_for(&self, room_id: i64, user_id: &str) {
        self.control_pending
            .remove_if(&room_id, |_, p| p.requester_id == user_id);
    }

    // ---- LC-494 stage control plane -----------------------------------

    /// Join `user_id` to stage `room_id` as a listener (idempotent).
    pub fn stage_join(&self, room_id: i64, user_id: &str) {
        self.stages
            .entry(room_id)
            .or_default()
            .participants
            .insert(user_id.to_string());
    }

    /// Remove `user_id` from stage `room_id` entirely (leave / disconnect).
    /// Drops the room entry once empty so an idle stage holds no state.
    pub fn stage_leave(&self, room_id: i64, user_id: &str) {
        if let Some(mut s) = self.stages.get_mut(&room_id) {
            s.participants.remove(user_id);
            s.speakers.remove(user_id);
            s.hands.remove(user_id);
        }
        self.stages.retain(|_, s| !s.participants.is_empty());
    }

    /// Remove `user_id` from every stage they are in. Called on the user's last
    /// disconnect (a person leaves the stage when their final tab closes).
    /// Returns the room ids they were removed from so the caller can broadcast a
    /// `StageChanged` to each.
    pub fn stage_leave_all(&self, user_id: &str) -> Vec<i64> {
        let mut affected = Vec::new();
        for mut s in self.stages.iter_mut() {
            if s.participants.remove(user_id) {
                affected.push(*s.key());
            }
            s.speakers.remove(user_id);
            s.hands.remove(user_id);
        }
        self.stages.retain(|_, s| !s.participants.is_empty());
        affected
    }

    /// Listener requests the floor. No-op if not a participant or already a
    /// speaker. Returns true if a hand was newly raised.
    pub fn stage_raise_hand(&self, room_id: i64, user_id: &str) -> bool {
        if let Some(mut s) = self.stages.get_mut(&room_id) {
            if s.participants.contains(user_id) && !s.speakers.contains(user_id) {
                return s.hands.insert(user_id.to_string());
            }
        }
        false
    }

    /// Withdraw a raised hand (idempotent).
    pub fn stage_lower_hand(&self, room_id: i64, user_id: &str) {
        if let Some(mut s) = self.stages.get_mut(&room_id) {
            s.hands.remove(user_id);
        }
    }

    /// Grant the floor (host action). Promotes a participant to speaker and
    /// clears any raised hand. No-op if they are not on the stage.
    pub fn stage_promote(&self, room_id: i64, user_id: &str) {
        if let Some(mut s) = self.stages.get_mut(&room_id) {
            if s.participants.contains(user_id) {
                s.speakers.insert(user_id.to_string());
                s.hands.remove(user_id);
            }
        }
    }

    /// Revoke the floor (host action, or a speaker stepping down): back to
    /// listener.
    pub fn stage_demote(&self, room_id: i64, user_id: &str) {
        if let Some(mut s) = self.stages.get_mut(&room_id) {
            s.speakers.remove(user_id);
        }
    }

    /// True when `user_id` is currently on stage `room_id`.
    pub fn stage_has_participant(&self, room_id: i64, user_id: &str) -> bool {
        self.stages
            .get(&room_id)
            .map(|s| s.participants.contains(user_id))
            .unwrap_or(false)
    }

    /// Snapshot of stage `room_id`'s roster, or `None` if no one is on it.
    pub fn stage_roster(&self, room_id: i64) -> Option<StageState> {
        self.stages.get(&room_id).map(|s| s.clone())
    }

    /// Register a new connection. Returns (conn_id, broadcast::Receiver,
    /// `is_first_for_user`). The boolean lets the caller broadcast a
    /// presence-change event when the user transitions from no live tabs to
    /// at least one without forcing the hub to know about persisted status.
    pub fn connect(
        &self,
        user_id: &str,
        username: &str,
    ) -> (ConnId, broadcast::Receiver<ChatEvent>, bool) {
        let id = next_conn_id();
        let (tx, rx) = broadcast::channel(64);
        self.connections.insert(
            id,
            Connection {
                user_id: user_id.to_string(),
                username: username.to_string(),
                tx,
            },
        );
        let mut entry = self.user_conns.entry(user_id.to_string()).or_default();
        let is_first = entry.is_empty();
        entry.insert(id);
        (id, rx, is_first)
    }

    /// Unregister a connection and remove from all rooms. Returns the user_id
    /// when this disconnect removed the user's last live connection so the
    /// caller can broadcast an offline presence event.
    pub fn disconnect(&self, conn_id: ConnId) -> Option<String> {
        let (_, conn) = self.connections.remove(&conn_id)?;
        let mut became_offline: Option<String> = None;
        if let Some(mut conns) = self.user_conns.get_mut(&conn.user_id) {
            conns.remove(&conn_id);
            if conns.is_empty() {
                became_offline = Some(conn.user_id.clone());
                drop(conns);
                self.user_conns.remove(&conn.user_id);
            }
        }
        self.rooms.iter_mut().for_each(|mut entry| {
            entry.value_mut().remove(&conn_id);
        });
        self.rooms.retain(|_, conns| !conns.is_empty());
        self.topics.iter_mut().for_each(|mut entry| {
            entry.value_mut().remove(&conn_id);
        });
        self.topics.retain(|_, conns| !conns.is_empty());
        became_offline
    }

    /// Subscribe a connection to a room.
    pub fn subscribe(&self, conn_id: ConnId, room_id: i64) {
        self.rooms.entry(room_id).or_default().insert(conn_id);
    }

    /// Unsubscribe a connection from a room.
    pub fn unsubscribe(&self, conn_id: ConnId, room_id: i64) {
        if let Some(mut conns) = self.rooms.get_mut(&room_id) {
            conns.remove(&conn_id);
        }
    }

    /// LC-160: subscribe a connection to a typed topic (caller authorizes
    /// first). Re-subscribing is idempotent (the set dedups).
    pub fn subscribe_topic(&self, conn_id: ConnId, topic: &str) {
        self.topics
            .entry(topic.to_string())
            .or_default()
            .insert(conn_id);
    }

    /// LC-176: unsubscribe a single connection from a typed topic. Idempotent.
    pub fn unsubscribe_topic(&self, conn_id: ConnId, topic: &str) {
        if let Some(mut conns) = self.topics.get_mut(topic) {
            conns.remove(&conn_id);
        }
    }

    /// LC-176: drop every one of a user's connections from `topic`, so a user
    /// who just lost access (kicked from / left / had-deleted an enclave) stops
    /// receiving that topic's events immediately instead of leaking updates
    /// until they happen to navigate. No-op if the user has no live
    /// connections. Connection ids are collected before mutating `topics` so no
    /// `user_conns` borrow is held across the topic update.
    pub fn unsubscribe_user_from_topic(&self, user_id: &str, topic: &str) {
        let conn_ids: Vec<ConnId> = self
            .user_conns
            .get(user_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        for conn_id in conn_ids {
            self.unsubscribe_topic(conn_id, topic);
        }
    }

    /// LC-160: broadcast an event to every connection subscribed to `topic`.
    pub fn broadcast_to_topic(&self, topic: &str, event: &ChatEvent) {
        if let Some(conns) = self.topics.get(topic) {
            for &conn_id in conns.iter() {
                if let Some(conn) = self.connections.get(&conn_id) {
                    let _ = conn.tx.send(event.clone());
                }
            }
        }
    }

    /// Broadcast an event to all connections subscribed to a room.
    pub fn broadcast_to_room(&self, room_id: i64, event: &ChatEvent) {
        if let Some(conns) = self.rooms.get(&room_id) {
            for &conn_id in conns.iter() {
                if let Some(conn) = self.connections.get(&conn_id) {
                    let _ = conn.tx.send(event.clone());
                }
            }
        }
    }

    /// Broadcast an event to all room subscribers except one connection (the sender).
    pub fn broadcast_to_room_except(
        &self,
        room_id: i64,
        event: &ChatEvent,
        except_conn_id: ConnId,
    ) {
        if let Some(conns) = self.rooms.get(&room_id) {
            for &conn_id in conns.iter() {
                if conn_id == except_conn_id {
                    continue;
                }
                if let Some(conn) = self.connections.get(&conn_id) {
                    let _ = conn.tx.send(event.clone());
                }
            }
        }
    }

    /// Broadcast an event to ALL connected users (for global mod events like ban/mute).
    pub fn broadcast_global(&self, event: &ChatEvent) {
        for entry in self.connections.iter() {
            let _ = entry.value().tx.send(event.clone());
        }
    }

    /// Return the user_ids of every connected user (deduped). Used to fan out
    /// public-room sidebar updates to all live sessions.
    pub fn list_connected_users(&self) -> Vec<String> {
        self.user_conns.iter().map(|e| e.key().clone()).collect()
    }

    /// True when `user_id` has at least one open WebSocket. Used by view
    /// loaders to render an "offline" status circle for users with no live
    /// session, regardless of their persisted status enum.
    pub fn is_user_connected(&self, user_id: &str) -> bool {
        self.user_conns.get(user_id).is_some_and(|c| !c.is_empty())
    }

    /// Broadcast to a specific user's connections (all their tabs/devices).
    pub fn broadcast_to_user(&self, user_id: &str, event: &ChatEvent) {
        if let Some(conns) = self.user_conns.get(user_id) {
            for &conn_id in conns.iter() {
                if let Some(conn) = self.connections.get(&conn_id) {
                    let _ = conn.tx.send(event.clone());
                }
            }
        }
    }

    /// LC-838: a rename reached the live record; keep this connection's copy
    /// of the display label (read by the typing, wiki-editing and voice roster
    /// paths) in step with it. No-op for an unknown id.
    pub fn set_username(&self, conn_id: ConnId, username: &str) {
        if let Some(mut conn) = self.connections.get_mut(&conn_id) {
            conn.username = username.to_string();
        }
    }

    /// The display label a connection was registered (or last renamed) with.
    pub fn username_of(&self, conn_id: ConnId) -> Option<String> {
        self.connections.get(&conn_id).map(|c| c.username.clone())
    }

    /// LC-836: deliver `event` to exactly one connection. For state that is
    /// per-connection rather than per-user (which page a tab is on), a
    /// `broadcast_to_user` would make every other tab of the same user
    /// re-render for a move it did not make. No-op for an unknown id.
    pub fn send_to_conn(&self, conn_id: ConnId, event: &ChatEvent) {
        if let Some(conn) = self.connections.get(&conn_id) {
            let _ = conn.tx.send(event.clone());
        }
    }

    /// Record a typing event for a connection. Broadcasts UserTyping to the room
    /// (excluding the sender) on the first frame of a new typing session, then
    /// spawns an eviction task that sends UserStoppedTyping after 5s of silence.
    pub fn notify_typing(self: &Arc<Self>, conn_id: ConnId, room_id: i64) {
        let (user_id, username) = match self.connections.get(&conn_id) {
            Some(c) => (c.user_id.clone(), c.username.clone()),
            None => return,
        };

        let key = (room_id, user_id.clone());
        let is_new = !self.typing.contains_key(&key);
        self.typing.insert(key.clone(), std::time::Instant::now());

        // Only broadcast on the first frame of a new typing session.
        if is_new {
            let event = ChatEvent::UserTyping {
                room_id,
                user_id: user_id.clone(),
                username,
            };
            self.broadcast_to_room_except(room_id, &event, conn_id);
        }

        // Spawn eviction task: after 5s check if the user has gone silent.
        let hub = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(entry) = hub.typing.get(&key) {
                if entry.elapsed() >= std::time::Duration::from_secs(5) {
                    drop(entry);
                    hub.stop_typing(room_id, &user_id);
                }
            }
        });
    }

    /// Remove a user's typing state for a room and broadcast UserStoppedTyping.
    pub fn stop_typing(&self, room_id: i64, user_id: &str) {
        let key = (room_id, user_id.to_string());
        if self.typing.remove(&key).is_some() {
            let event = ChatEvent::UserStoppedTyping {
                room_id,
                user_id: user_id.to_string(),
            };
            self.broadcast_to_room(room_id, &event);
        }
    }

    /// LC-498: record a "editing the wiki" heartbeat for a connection. Mirrors
    /// `notify_typing`: broadcasts `WikiEditing` to the room (excluding the
    /// editor) on the first frame, then evicts after 5s of silence. The client
    /// heartbeats every ~3s while the wiki editor is focused, so the banner
    /// stays up through reading/thinking pauses and clears shortly after the
    /// editor blurs, saves, or navigates away.
    pub fn notify_wiki_editing(self: &Arc<Self>, conn_id: ConnId, room_id: i64) {
        let (user_id, username) = match self.connections.get(&conn_id) {
            Some(c) => (c.user_id.clone(), c.username.clone()),
            None => return,
        };

        let key = (room_id, user_id.clone());
        let is_new = !self.wiki_editing.contains_key(&key);
        self.wiki_editing
            .insert(key.clone(), std::time::Instant::now());

        if is_new {
            let event = ChatEvent::WikiEditing {
                room_id,
                user_id: user_id.clone(),
                username,
            };
            self.broadcast_to_room_except(room_id, &event, conn_id);
        }

        let hub = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(entry) = hub.wiki_editing.get(&key) {
                if entry.elapsed() >= std::time::Duration::from_secs(5) {
                    drop(entry);
                    hub.stop_wiki_editing(room_id, &user_id);
                }
            }
        });
    }

    /// Remove a user's wiki-editing state for a room and broadcast
    /// `WikiStoppedEditing` so viewers clear the banner.
    pub fn stop_wiki_editing(&self, room_id: i64, user_id: &str) {
        let key = (room_id, user_id.to_string());
        if self.wiki_editing.remove(&key).is_some() {
            let event = ChatEvent::WikiStoppedEditing {
                room_id,
                user_id: user_id.to_string(),
            };
            self.broadcast_to_room(room_id, &event);
        }
    }

    /// Thread-scoped typing. Mirrors `notify_typing` but keys on parent_id so
    /// the thread panel and room composer have distinct indicator state.
    pub fn notify_thread_typing(self: &Arc<Self>, conn_id: ConnId, room_id: i64, parent_id: i64) {
        let (user_id, username) = match self.connections.get(&conn_id) {
            Some(c) => (c.user_id.clone(), c.username.clone()),
            None => return,
        };

        let key = (room_id, parent_id, user_id.clone());
        let is_new = !self.thread_typing.contains_key(&key);
        self.thread_typing
            .insert(key.clone(), std::time::Instant::now());

        if is_new {
            let event = ChatEvent::ThreadTyping {
                room_id,
                parent_id,
                user_id: user_id.clone(),
                username,
            };
            self.broadcast_to_room_except(room_id, &event, conn_id);
        }

        let hub = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(entry) = hub.thread_typing.get(&key) {
                if entry.elapsed() >= std::time::Duration::from_secs(5) {
                    drop(entry);
                    hub.stop_thread_typing(room_id, parent_id, &user_id);
                }
            }
        });
    }

    pub fn stop_thread_typing(&self, room_id: i64, parent_id: i64, user_id: &str) {
        let key = (room_id, parent_id, user_id.to_string());
        if self.thread_typing.remove(&key).is_some() {
            let event = ChatEvent::ThreadStoppedTyping {
                room_id,
                parent_id,
                user_id: user_id.to_string(),
            };
            self.broadcast_to_room(room_id, &event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // LC-176: a user dropped from a topic stops receiving its events on every
    // one of their connections, while other users on the topic are unaffected.
    #[test]
    fn unsubscribe_user_from_topic_stops_delivery_for_that_user_only() {
        let hub = Hub::new();
        let (c1, mut rx1, _) = hub.connect("u1", "u1");
        let (c2, mut rx2, _) = hub.connect("u1", "u1"); // u1's second tab
        let (c3, mut rx3, _) = hub.connect("u2", "u2");
        let topic = "enclave:5";
        hub.subscribe_topic(c1, topic);
        hub.subscribe_topic(c2, topic);
        hub.subscribe_topic(c3, topic);
        let evt = ChatEvent::EnclaveMemberRemoved {
            enclave_id: 5,
            user_id: "x".into(),
        };

        hub.broadcast_to_topic(topic, &evt);
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
        assert!(rx3.try_recv().is_ok());

        hub.unsubscribe_user_from_topic("u1", topic);
        hub.broadcast_to_topic(topic, &evt);
        assert!(
            rx1.try_recv().is_err(),
            "u1 tab 1 must not receive after unsubscribe"
        );
        assert!(
            rx2.try_recv().is_err(),
            "u1 tab 2 must not receive after unsubscribe"
        );
        assert!(rx3.try_recv().is_ok(), "u2 stays subscribed");
    }

    #[test]
    fn unsubscribe_user_from_topic_is_noop_for_unknown_user() {
        let hub = Hub::new();
        // No connections / no subscription; must not panic.
        hub.unsubscribe_user_from_topic("ghost", "enclave:1");
    }

    // LC-498: a wiki-editing heartbeat broadcasts WikiEditing to the room except
    // the editor's own connection; stop_wiki_editing broadcasts WikiStoppedEditing
    // to everyone (incl. the editor) and is idempotent.
    #[tokio::test]
    async fn wiki_editing_presence_excludes_editor_and_clears() {
        let hub = std::sync::Arc::new(Hub::new());
        let (editor, mut editor_rx, _) = hub.connect("u1", "alice");
        let (viewer, mut viewer_rx, _) = hub.connect("u2", "bob");
        hub.subscribe(editor, 7);
        hub.subscribe(viewer, 7);

        hub.notify_wiki_editing(editor, 7);
        // Viewer sees the banner event; editor does not (except-self).
        assert!(
            matches!(
                viewer_rx.try_recv(),
                Ok(ChatEvent::WikiEditing { room_id: 7, .. })
            ),
            "viewer receives WikiEditing"
        );
        assert!(
            editor_rx.try_recv().is_err(),
            "editor must not receive its own WikiEditing"
        );

        // A second heartbeat is not a new session, so no duplicate broadcast.
        hub.notify_wiki_editing(editor, 7);
        assert!(
            viewer_rx.try_recv().is_err(),
            "no re-broadcast while the session is already live"
        );

        hub.stop_wiki_editing(7, "u1");
        assert!(
            matches!(
                viewer_rx.try_recv(),
                Ok(ChatEvent::WikiStoppedEditing { room_id: 7, .. })
            ),
            "viewer receives WikiStoppedEditing"
        );
        assert!(
            matches!(
                editor_rx.try_recv(),
                Ok(ChatEvent::WikiStoppedEditing { room_id: 7, .. })
            ),
            "stop is broadcast to the whole room incl. the editor"
        );

        // Idempotent: a second stop with no live session broadcasts nothing.
        hub.stop_wiki_editing(7, "u1");
        assert!(viewer_rx.try_recv().is_err());
    }
}
