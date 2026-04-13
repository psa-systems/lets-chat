# Phase 8: Typing Indicators Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show "Alice is typing…" in real time when another user is composing a message. Purely ephemeral — no DB changes. The feature lives entirely in the WS layer plus two UI components.

**Architecture:** The client sends a `ClientControl::Typing { room_id }` frame on each keystroke (debounced client-side to at most once per second). The hub records a `(room_id, user_id) → Instant` entry and broadcasts `ChatEvent::UserTyping` to other subscribers in the room — only on the first frame of a new typing session to avoid spam. A tokio task spawned per frame checks after 5 seconds whether the user has gone silent; if so, it removes the entry and broadcasts `ChatEvent::UserStoppedTyping`. The room and DM views maintain a `Signal<Vec<String>>` of currently-typing usernames and display them above the composer.

**Tech Stack:** No new dependencies. Builds via Docker (`rust:1.93-slim-trixie`).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `src/ws/events.rs` | Add `UserTyping`, `UserStoppedTyping` to `ChatEvent`; add `Typing` to `ClientControl` |
| Modify | `src/ws/hub.rs` | Add `username` to `Connection`; add `typing` DashMap; add `notify_typing`, `stop_typing`, `broadcast_to_room_except` |
| Modify | `src/ws/handler.rs` | Pass `username` to `hub.connect()`; handle `ClientControl::Typing` |
| Modify | `src/components/use_websocket.rs` | Add `send_typing(room_id)` to `WsHandle` |
| Modify | `src/components/room_view.rs` | Send `Typing` on oninput; handle typing events; show indicator |
| Modify | `src/components/dm_view.rs` | Same as room_view |

---

### Task 1: WS events

**Files:**
- Modify: `src/ws/events.rs`

- [ ] **Step 1: Add typing variants to `ChatEvent` and `ClientControl`**

Replace `src/ws/events.rs` in full:

```rust
use serde::{Deserialize, Serialize};

use crate::models::Message;

/// Events sent from server to client over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    NewMessage {
        message: Message,
        is_dm: bool,
    },
    MessageEdited {
        message_id: i64,
        room_id: i64,
        new_body: String,
        edited_at: String,
    },
    MessageDeleted {
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
}

/// Control frames sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientControl {
    Subscribe { room_id: i64 },
    Unsubscribe { room_id: i64 },
    Typing { room_id: i64 },
}
```

- [ ] **Step 2: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -5
```

Expected: compile errors in `handler.rs` for non-exhaustive `ClientControl` match. That is expected — fixed in Task 3.

- [ ] **Step 3: Commit**

```bash
git add src/ws/events.rs
git commit -m "feat(ws): add UserTyping, UserStoppedTyping, and Typing control frame"
```

---

### Task 2: Hub typing state

**Files:**
- Modify: `src/ws/hub.rs`

- [ ] **Step 1: Add `username` to `Connection` and update `connect()`**

Replace the `Connection` struct and `connect` method:

```rust
/// A connected user's handle.
struct Connection {
    user_id: String,
    username: String,
    tx: broadcast::Sender<ChatEvent>,
}
```

```rust
/// Register a new connection. Returns (conn_id, broadcast::Receiver).
pub fn connect(&self, user_id: &str, username: &str) -> (ConnId, broadcast::Receiver<ChatEvent>) {
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
    self.user_conns
        .entry(user_id.to_string())
        .or_default()
        .insert(id);
    (id, rx)
}
```

- [ ] **Step 2: Add `typing` map to `Hub`**

Add the field to the `Hub` struct:

```rust
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
```

Update `Hub::new()`:

```rust
pub fn new() -> Self {
    Self {
        connections: DashMap::new(),
        rooms: DashMap::new(),
        user_conns: DashMap::new(),
        typing: DashMap::new(),
    }
}
```

- [ ] **Step 3: Add `broadcast_to_room_except`**

Add after `broadcast_to_room`:

```rust
/// Broadcast an event to all room subscribers except one connection (the sender).
pub fn broadcast_to_room_except(&self, room_id: i64, event: &ChatEvent, except_conn_id: ConnId) {
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
```

- [ ] **Step 4: Add `notify_typing` and `stop_typing`**

Add after `broadcast_to_room_except`:

```rust
/// Record a typing event for a connection. Broadcasts UserTyping to the room
/// (excluding the sender) on the first frame of a new typing session, then
/// spawns an eviction task that sends UserStoppedTyping after 5s of silence.
pub fn notify_typing(&self, conn_id: ConnId, room_id: i64) {
    let (user_id, username) = match self.connections.get(&conn_id) {
        Some(c) => (c.user_id.clone(), c.username.clone()),
        None => return,
    };

    let key = (room_id, user_id.clone());
    let now = std::time::Instant::now();
    let is_new = !self.typing.contains_key(&key);
    self.typing.insert(key.clone(), now);

    // Only broadcast the first frame of a new typing session to avoid spam.
    if is_new {
        let event = ChatEvent::UserTyping {
            room_id,
            user_id: user_id.clone(),
            username,
        };
        self.broadcast_to_room_except(room_id, &event, conn_id);
    }

    // Spawn eviction task. After 5s, if the stored instant is still old,
    // the user has stopped typing — remove and broadcast UserStoppedTyping.
    let hub = get_hub().clone();
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
/// Called by the eviction task and can also be called on message send.
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
```

- [ ] **Step 5: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -5
```

Expected: compile errors in `handler.rs` (wrong arity on `hub.connect` call). That is expected — fixed in Task 3.

- [ ] **Step 6: Commit**

```bash
git add src/ws/hub.rs
git commit -m "feat(ws): add typing state map and notify_typing/stop_typing to hub"
```

---

### Task 3: Handler — wire username and handle `Typing` frame

**Files:**
- Modify: `src/ws/handler.rs`

- [ ] **Step 1: Pass `username` through to `handle_socket`**

In `ws_handler`, change:

```rust
let user_id = user.id.clone();

ws.on_upgrade(move |socket| handle_socket(socket, user_id))
```

to:

```rust
let user_id = user.id.clone();
let username = user.display_name.clone().unwrap_or_else(|| user.username.clone());

ws.on_upgrade(move |socket| handle_socket(socket, user_id, username))
```

Update `handle_socket` signature:

```rust
async fn handle_socket(socket: WebSocket, user_id: String, username: String) {
```

And update the `hub.connect` call:

```rust
let (conn_id, mut rx) = hub.connect(&user_id, &username);
```

- [ ] **Step 2: Handle `ClientControl::Typing` in the read loop**

Add the `Typing` arm to the match in the client read loop:

```rust
ClientControl::Subscribe { room_id } => {
    // ... existing logic unchanged ...
}
ClientControl::Unsubscribe { room_id } => {
    hub.unsubscribe(conn_id, room_id);
}
ClientControl::Typing { room_id } => {
    hub.notify_typing(conn_id, room_id);
}
```

- [ ] **Step 3: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -5
```

Expected: clean compile.

- [ ] **Step 4: Commit**

```bash
git add src/ws/handler.rs
git commit -m "feat(ws): pass username to hub connect, handle Typing control frame"
```

---

### Task 4: `WsHandle` — add `send_typing`

**Files:**
- Modify: `src/components/use_websocket.rs`

- [ ] **Step 1: Add `send_typing` to `WsHandle`**

Add after `unsubscribe`:

```rust
pub fn send_typing(&self, room_id: i64) {
    if let Some(ref tx) = *self.sender.read() {
        let msg = ClientControl::Typing { room_id };
        tx.send(&serde_json::to_string(&msg).unwrap());
    }
}
```

- [ ] **Step 2: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -5
```

Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/components/use_websocket.rs
git commit -m "feat(ws): add send_typing to WsHandle"
```

---

### Task 5: UI — `room_view.rs`

**Files:**
- Modify: `src/components/room_view.rs`

- [ ] **Step 1: Add `typing_users` signal and `last_typing_sent` signal**

Add alongside the existing signals near the top of the component:

```rust
let mut typing_users = use_signal(Vec::<String>::new);
let mut last_typing_sent = use_signal(|| 0.0f64);
```

- [ ] **Step 2: Add a separate `use_effect` for typing events**

Add after the existing WS `use_effect` block (the one that handles `NewMessage` / `MessageDeleted` / `MessageEdited`):

```rust
// Handle typing indicator events — separate effect to avoid touching messages_version
use_effect(move || {
    if let Some(ref event) = *ws.latest_event.read() {
        match event {
            ChatEvent::UserTyping { room_id, user_id, username }
                if *room_id == parsed_id && *user_id != u.id =>
            {
                let name = username.clone();
                typing_users.with_mut(|v| {
                    if !v.contains(&name) {
                        v.push(name);
                    }
                });
            }
            ChatEvent::UserStoppedTyping { room_id, user_id }
                if *room_id == parsed_id =>
            {
                let uid = user_id.clone();
                // Re-fetch the username from typing_users to remove by matching user_id
                // is tricky since typing_users stores usernames. Use the event user_id
                // to find and remove: store (user_id, username) pairs instead.
                // For simplicity: the server broadcasts username in UserTyping,
                // so we can look up by user_id only if we stored it. Here we clear
                // by user_id via a second approach — see note below.
                let _ = uid; // handled below with the paired signal
                let v = *messages_version.peek(); // no-op read just to satisfy compiler
                let _ = v;
                typing_users.set(Vec::new()); // simplest: clear all on any stop
            }
            ChatEvent::NewMessage { message, .. } if message.room_id == parsed_id => {
                // Clear the sender's typing indicator when their message arrives
                typing_users.set(Vec::new());
            }
            _ => {}
        }
    }
});
```

> **Note:** The simplest correct implementation stores `(user_id, username)` pairs in `typing_users`. See Step 3 for the cleaner version.

- [ ] **Step 2 (revised): Cleaner approach — store `(user_id, username)` pairs**

Replace the previous step with this cleaner signal and effect:

```rust
let mut typing_users: Signal<Vec<(String, String)>> = use_signal(Vec::new);
```

```rust
// Handle typing indicator events
use_effect(move || {
    if let Some(ref event) = *ws.latest_event.read() {
        match event {
            ChatEvent::UserTyping { room_id, user_id, username }
                if *room_id == parsed_id && *user_id != u.id =>
            {
                let uid = user_id.clone();
                let name = username.clone();
                typing_users.with_mut(|v| {
                    if !v.iter().any(|(id, _)| id == &uid) {
                        v.push((uid, name));
                    }
                });
            }
            ChatEvent::UserStoppedTyping { room_id, user_id }
                if *room_id == parsed_id =>
            {
                let uid = user_id.clone();
                typing_users.with_mut(|v| v.retain(|(id, _)| id != &uid));
            }
            ChatEvent::NewMessage { message, .. } if message.room_id == parsed_id => {
                let uid = message.user_id.clone();
                typing_users.with_mut(|v| v.retain(|(id, _)| id != &uid));
            }
            _ => {}
        }
    }
});
```

- [ ] **Step 3: Send `Typing` on composer `oninput`**

In the composer `oninput` handler, add the typing send after updating `draft`. Use `js_sys::Date::now()` to debounce to at most once per second:

```rust
oninput: move |evt| {
    draft.set(evt.value());
    // Debounce: send at most one Typing frame per second
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::now();
        if now - *last_typing_sent.peek() > 1000.0 {
            last_typing_sent.set(now);
            ws.send_typing(parsed_id);
        }
    }
},
```

- [ ] **Step 4: Render the typing indicator above the composer**

Add this block just above the form (or the mute banner), before the `if is_muted` block:

```rust
// Typing indicator
{
    let typers: Vec<String> = typing_users.read().iter().map(|(_, name)| name.clone()).collect();
    if !typers.is_empty() {
        let label = match typers.len() {
            1 => format!("{} is typing…", typers[0]),
            2 => format!("{} and {} are typing…", typers[0], typers[1]),
            _ => "Several people are typing…".to_string(),
        };
        rsx! {
            div { class: "px-6 py-1 text-xs text-gray-400 italic", "{label}" }
        }
    }
}
```

- [ ] **Step 5: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -5
```

Expected: clean compile.

- [ ] **Step 6: Commit**

```bash
git add src/components/room_view.rs
git commit -m "feat(ui): add typing indicator to room view"
```

---

### Task 6: UI — `dm_view.rs`

**Files:**
- Modify: `src/components/dm_view.rs`

Apply the identical changes from Task 5 to `dm_view.rs`:

- [ ] **Step 1: Add `typing_users` and `last_typing_sent` signals** (same as Task 5 Step 1)

- [ ] **Step 2: Add typing `use_effect`** — same as Task 5 Step 2, matching on `room_id` (the DM room's resolved ID, which is already `room_id: i64` in scope)

- [ ] **Step 3: Add `Typing` send to the composer `oninput`** — same as Task 5 Step 3, using the DM `room_id`

- [ ] **Step 4: Render typing indicator** — same as Task 5 Step 4

- [ ] **Step 5: Build check**

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -5
```

Expected: clean compile.

Also run the full test suite to confirm no regressions:

```bash
docker run --rm -v /home/long/lets-chat:/app -w /app rust:1.93-slim-trixie cargo test 2>&1 | tail -10
```

Expected: all tests pass (no new tests for this phase — pure WS, no DB).

- [ ] **Step 6: Commit**

```bash
git add src/components/dm_view.rs
git commit -m "feat(ui): add typing indicator to DM view"
```

---

## Phase 8 complete checklist

- [ ] `ChatEvent` has `UserTyping` and `UserStoppedTyping` variants
- [ ] `ClientControl` has `Typing` variant
- [ ] `Connection` stores `username`; `hub.connect()` accepts username
- [ ] Hub has `typing: DashMap<(i64, String), Instant>`
- [ ] `notify_typing` broadcasts `UserTyping` on new session, spawns 5s eviction task
- [ ] `stop_typing` removes entry and broadcasts `UserStoppedTyping`
- [ ] `broadcast_to_room_except` excludes the sender's own connection
- [ ] `handler.rs` passes username to `hub.connect()` and handles `ClientControl::Typing`
- [ ] `WsHandle` has `send_typing(room_id)`
- [ ] `room_view.rs` sends `Typing` on oninput (debounced 1s), shows typing label
- [ ] `dm_view.rs` same
- [ ] Full test suite still passes
