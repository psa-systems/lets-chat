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
}

impl Hub {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            rooms: DashMap::new(),
            user_conns: DashMap::new(),
        }
    }

    /// Register a new connection. Returns (conn_id, broadcast::Receiver).
    pub fn connect(&self, user_id: &str) -> (ConnId, broadcast::Receiver<ChatEvent>) {
        let id = next_conn_id();
        let (tx, rx) = broadcast::channel(64);
        self.connections.insert(
            id,
            Connection {
                user_id: user_id.to_string(),
                tx,
            },
        );
        self.user_conns
            .entry(user_id.to_string())
            .or_default()
            .insert(id);
        (id, rx)
    }

    /// Unregister a connection and remove from all rooms.
    pub fn disconnect(&self, conn_id: ConnId) {
        if let Some((_, conn)) = self.connections.remove(&conn_id) {
            // Remove from user_conns
            if let Some(mut conns) = self.user_conns.get_mut(&conn.user_id) {
                conns.remove(&conn_id);
                if conns.is_empty() {
                    drop(conns);
                    self.user_conns.remove(&conn.user_id);
                }
            }
            // Remove from all rooms
            self.rooms.iter_mut().for_each(|mut entry| {
                entry.value_mut().remove(&conn_id);
            });
            // Clean up empty rooms
            self.rooms.retain(|_, conns| !conns.is_empty());
        }
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

    /// Broadcast an event to ALL connected users (for global mod events like ban/mute).
    pub fn broadcast_global(&self, event: &ChatEvent) {
        for entry in self.connections.iter() {
            let _ = entry.value().tx.send(event.clone());
        }
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
}

/// Global hub instance.
static HUB: std::sync::OnceLock<Arc<Hub>> = std::sync::OnceLock::new();

pub fn get_hub() -> &'static Arc<Hub> {
    HUB.get_or_init(|| Arc::new(Hub::new()))
}
