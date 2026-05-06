use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::ws::events::ChatEvent;

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
    /// user_id -> set of conn_ids (a user may have multiple tabs)
    user_conns: DashMap<String, HashSet<ConnId>>,
    /// (room_id, user_id) -> last typing Instant (ephemeral, no DB)
    typing: DashMap<(i64, String), std::time::Instant>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            rooms: DashMap::new(),
            user_conns: DashMap::new(),
            typing: DashMap::new(),
        }
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
}
