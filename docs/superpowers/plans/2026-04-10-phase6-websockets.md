# Phase 6: WebSockets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real-time server-to-client event delivery via WebSockets so chat messages, moderation actions, and DM notifications appear instantly without polling.

**Architecture:** A dedicated `/ws` Axum endpoint authenticates via session cookie, then holds a long-lived connection per user. An in-memory hub (`DashMap<i64, HashSet<ConnectionId>>`) tracks which connections are subscribed to which rooms. Server functions (send_message, delete_message, etc.) broadcast events through the hub after writing to the DB. Clients send `Subscribe`/`Unsubscribe` control frames as users navigate. The WebSocket is server-to-client fan-out only — sending messages stays as HTTP server functions.

**Tech Stack:** Axum 0.8 built-in WebSocket (`axum::extract::ws`), `tokio::sync::broadcast` for per-connection event delivery, `dashmap` for concurrent connection registry, `futures` for stream splitting. All builds via Docker (`rust:1.93-slim-trixie`).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src/ws/mod.rs` | Module root, re-exports |
| Create | `src/ws/events.rs` | `ChatEvent` enum (serde-serializable) |
| Create | `src/ws/hub.rs` | In-memory connection registry + broadcast |
| Create | `src/ws/handler.rs` | `/ws` Axum endpoint handler (upgrade, auth, read/write loop) |
| Modify | `Cargo.toml` | Add `dashmap`, `futures` deps |
| Modify | `src/main.rs` | Register `ws` module, mount `/ws` route |
| Modify | `src/server_fns/chat.rs` | Broadcast `NewMessage` after insert |
| Modify | `src/server_fns/dm.rs` | Broadcast `NewMessage` after DM insert |
| Modify | `src/server_fns/moderation.rs` | Broadcast mod events after each action |
| Create | `src/components/use_websocket.rs` | Client-side `use_websocket()` hook |
| Modify | `src/components/mod.rs` | Add `use_websocket` module |
| Modify | `src/components/auth_layout.rs` | Initialize WebSocket hook at login |
| Modify | `src/components/room_view.rs` | Subscribe to room, append messages from WS |
| Modify | `src/components/dm_view.rs` | Subscribe to DM room, append messages from WS |
| Modify | `src/components/sidebar.rs` | Listen for new DM events to refresh sidebar |

---

### Task 1: ChatEvent enum and models

**Files:**
- Create: `src/ws/events.rs`
- Create: `src/ws/mod.rs`
- Modify: `src/main.rs` (add `mod ws`)

- [ ] **Step 1: Create `src/ws/events.rs` with the ChatEvent enum**

```rust
use serde::{Deserialize, Serialize};

use crate::models::Message;

/// Events sent from server to client over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    NewMessage {
        message: Message,
    },
    MessageDeleted {
        message_id: i64,
        room_id: i64,
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
}
```

- [ ] **Step 2: Create `src/ws/mod.rs`**

```rust
pub mod events;
#[cfg(not(target_arch = "wasm32"))]
pub mod handler;
#[cfg(not(target_arch = "wasm32"))]
pub mod hub;
```

- [ ] **Step 3: Add `mod ws` to `src/main.rs`**

Add after `mod server_fns;`:
```rust
mod ws;
```

- [ ] **Step 4: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles (warnings OK, no errors). The `handler` and `hub` modules don't exist yet so the `#[cfg(not(target_arch = "wasm32"))]` gates mean they're only needed server-side — but we should create stub files so the compile succeeds.

Actually, since the modules are declared but files don't exist, create empty stubs:

Create `src/ws/hub.rs`:
```rust
// Hub implementation in Task 2
```

Create `src/ws/handler.rs`:
```rust
// Handler implementation in Task 3
```

- [ ] **Step 5: Commit**

```bash
git add src/ws/
git commit -m "feat(ws): add ChatEvent enum and ws module structure"
```

---

### Task 2: Connection hub (in-memory registry + broadcast)

**Files:**
- Modify: `Cargo.toml` (add `dashmap`, `futures`)
- Create: `src/ws/hub.rs`

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

In the `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` section, add:
```toml
dashmap = "6"
futures = "0.3"
```

- [ ] **Step 2: Write `src/ws/hub.rs`**

```rust
use std::collections::{HashMap, HashSet};
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
```

- [ ] **Step 3: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles clean.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml src/ws/hub.rs
git commit -m "feat(ws): add connection hub with room subscriptions and broadcast"
```

---

### Task 3: WebSocket endpoint handler

**Files:**
- Create: `src/ws/handler.rs`

- [ ] **Step 1: Write `src/ws/handler.rs`**

```rust
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use http::HeaderMap;

use crate::ws::events::ClientControl;
use crate::ws::hub::get_hub;

/// Extract session ID from request headers (same logic as helpers::get_session_id).
fn session_from_headers(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(http::header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("session=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Axum handler for `/ws` — upgrades to WebSocket after auth check.
pub async fn ws_handler(ws: WebSocketUpgrade, headers: HeaderMap) -> impl IntoResponse {
    let session_id = match session_from_headers(&headers) {
        Some(id) => id,
        None => {
            return (http::StatusCode::UNAUTHORIZED, "Missing session cookie").into_response();
        }
    };

    let pool = crate::db::get_auth_pool().await;
    let user = match crate::db::auth::get_user_by_session(pool, &session_id).await {
        Ok(Some(u)) => u,
        _ => {
            return (http::StatusCode::UNAUTHORIZED, "Invalid session").into_response();
        }
    };

    if user.is_banned {
        return (http::StatusCode::FORBIDDEN, "Account banned").into_response();
    }

    let user_id = user.id.clone();

    ws.on_upgrade(move |socket| handle_socket(socket, user_id))
}

async fn handle_socket(socket: WebSocket, user_id: String) {
    let hub = get_hub();
    let (conn_id, mut rx) = hub.connect(&user_id);

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Spawn task: forward hub events to the WebSocket
    let send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let json = match serde_json::to_string(&event) {
                Ok(j) => j,
                Err(_) => continue,
            };
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Spawn task: ping every 30s
    let ping_task = tokio::spawn({
        // We need a separate sender for pings, but axum's split gives us one sender.
        // Instead, we'll handle ping via the read loop timeout.
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        }
    });

    // Read loop: handle client control frames
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(ctrl) = serde_json::from_str::<ClientControl>(&text) {
                    match ctrl {
                        ClientControl::Subscribe { room_id } => {
                            hub.subscribe(conn_id, room_id);
                        }
                        ClientControl::Unsubscribe { room_id } => {
                            hub.unsubscribe(conn_id, room_id);
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup
    hub.disconnect(conn_id);
    send_task.abort();
    ping_task.abort();
}
```

- [ ] **Step 2: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles clean.

- [ ] **Step 3: Commit**

```bash
git add src/ws/handler.rs
git commit -m "feat(ws): add /ws endpoint handler with auth and subscribe/unsubscribe"
```

---

### Task 4: Register `/ws` route on Axum router

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Modify `build_server_router()` in `src/main.rs`**

Change from:
```rust
#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
fn build_server_router() -> axum::Router {
    use dioxus::server::DioxusRouterExt;
    axum::Router::new().serve_dioxus_application(ServeConfig::new(), App)
}
```

To:
```rust
#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
fn build_server_router() -> axum::Router {
    use axum::routing::get;
    use dioxus::server::DioxusRouterExt;
    axum::Router::new()
        .route("/ws", get(ws::handler::ws_handler))
        .serve_dioxus_application(ServeConfig::new(), App)
}
```

The `/ws` route must come **before** `.serve_dioxus_application()` so it takes priority over the Dioxus catch-all.

- [ ] **Step 2: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles clean.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(ws): register /ws route on Axum router"
```

---

### Task 5: Broadcast events from server functions

**Files:**
- Modify: `src/server_fns/chat.rs` — broadcast `NewMessage` after `send_message`
- Modify: `src/server_fns/dm.rs` — broadcast `NewMessage` after `send_dm_message`
- Modify: `src/server_fns/moderation.rs` — broadcast mod events

- [ ] **Step 1: Modify `send_message` in `src/server_fns/chat.rs`**

After the `insert_message` call (which returns the message id), build a `Message` and broadcast it. Replace the end of `send_message` (after the mute check):

```rust
    let chat_pool = crate::db::get_chat_pool().await;
    let msg_id = crate::db::chat::insert_message(chat_pool, room_id, &user.id, &body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Broadcast via WebSocket
    let author_name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let event = crate::ws::events::ChatEvent::NewMessage {
        message: crate::models::Message {
            id: msg_id,
            room_id,
            user_id: user.id.clone(),
            author_name,
            body,
            created_at: now,
        },
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);

    Ok(msg_id)
```

- [ ] **Step 2: Modify `send_dm_message` in `src/server_fns/dm.rs`**

Same pattern. After `insert_message`, broadcast to the DM room. Replace the end of `send_dm_message` (after the membership check):

```rust
    let msg_id = crate::db::chat::insert_message(chat_pool, room_id, &user.id, &body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Broadcast via WebSocket
    let author_name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let event = crate::ws::events::ChatEvent::NewMessage {
        message: crate::models::Message {
            id: msg_id,
            room_id,
            user_id: user.id.clone(),
            author_name,
            body,
            created_at: now,
        },
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);

    Ok(msg_id)
```

- [ ] **Step 3: Modify moderation functions in `src/server_fns/moderation.rs`**

Add broadcast calls at the end of each function, right before `Ok(())`:

**`delete_message`** — after logging mod action:
```rust
    let event = crate::ws::events::ChatEvent::MessageDeleted {
        message_id,
        room_id,
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);
```

**`ban_user`** — after logging mod action:
```rust
    let event = crate::ws::events::ChatEvent::UserBanned {
        user_id: user_id.clone(),
    };
    crate::ws::hub::get_hub().broadcast_global(&event);
```

**`mute_user`** — after logging mod action:
```rust
    let event = crate::ws::events::ChatEvent::UserMuted {
        user_id: user_id.clone(),
        muted_until: until_opt.clone(),
    };
    crate::ws::hub::get_hub().broadcast_global(&event);
```

**`kick_user`** — after logging mod action:
```rust
    let event = crate::ws::events::ChatEvent::UserKicked {
        user_id: user_id.clone(),
        room_id,
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);
```

The `unban_user`, `unmute_user`, `suspend_user`, and `list_mod_actions` functions do not need broadcasts (unban/unmute take effect on next request; suspend already bans+deletes sessions; list is read-only).

- [ ] **Step 4: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles clean.

- [ ] **Step 5: Commit**

```bash
git add src/server_fns/chat.rs src/server_fns/dm.rs src/server_fns/moderation.rs
git commit -m "feat(ws): broadcast events from send_message, send_dm_message, and moderation actions"
```

---

### Task 6: Client-side `use_websocket` hook

**Files:**
- Create: `src/components/use_websocket.rs`
- Modify: `src/components/mod.rs`

This is the client-side (WASM) hook. It uses the browser's `web_sys::WebSocket` API through Dioxus's WASM environment.

- [ ] **Step 1: Create `src/components/use_websocket.rs`**

```rust
use dioxus::prelude::*;

use crate::ws::events::{ChatEvent, ClientControl};

/// Provides a reactive stream of ChatEvent from the server WebSocket.
/// Call `subscribe(room_id)` / `unsubscribe(room_id)` as user navigates.
#[derive(Clone)]
pub struct WsHandle {
    /// Latest event received (components read this reactively).
    pub latest_event: Signal<Option<ChatEvent>>,
    /// Send a control frame to the server.
    sender: Signal<Option<WebSocketSender>>,
}

impl WsHandle {
    pub fn subscribe(&self, room_id: i64) {
        if let Some(ref tx) = *self.sender.read() {
            let msg = ClientControl::Subscribe { room_id };
            tx.send(&serde_json::to_string(&msg).unwrap());
        }
    }

    pub fn unsubscribe(&self, room_id: i64) {
        if let Some(ref tx) = *self.sender.read() {
            let msg = ClientControl::Unsubscribe { room_id };
            tx.send(&serde_json::to_string(&msg).unwrap());
        }
    }
}

#[derive(Clone)]
struct WebSocketSender {
    #[cfg(target_arch = "wasm32")]
    ws: web_sys::WebSocket,
}

impl WebSocketSender {
    fn send(&self, msg: &str) {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self.ws.send_with_str(msg);
        }
    }
}

/// Initialize WebSocket connection. Call once in AuthLayout.
/// Returns a WsHandle that should be provided as context.
pub fn use_websocket() -> WsHandle {
    let mut latest_event = use_signal(|| None::<ChatEvent>);
    let mut sender = use_signal(|| None::<WebSocketSender>);

    #[cfg(target_arch = "wasm32")]
    {
        use_effect(move || {
            spawn(async move {
                connect_ws(latest_event, sender).await;
            });
        });
    }

    WsHandle {
        latest_event,
        sender,
    }
}

#[cfg(target_arch = "wasm32")]
async fn connect_ws(
    mut latest_event: Signal<Option<ChatEvent>>,
    mut sender: Signal<Option<WebSocketSender>>,
) {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{MessageEvent, WebSocket};

    let window = web_sys::window().unwrap();
    let location = window.location();
    let protocol = if location.protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let host = location.host().unwrap_or_else(|_| "localhost:8080".into());
    let url = format!("{}://{}/ws", protocol, host);

    let mut backoff_ms: u32 = 500;
    let max_backoff_ms: u32 = 30_000;

    loop {
        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(_) => {
                gloo_timers::future::TimeoutFuture::new(backoff_ms).await;
                backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
                continue;
            }
        };

        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // Wait for open
        let (open_tx, open_rx) = futures::channel::oneshot::channel::<bool>();
        let open_tx = std::cell::RefCell::new(Some(open_tx));
        let onopen = Closure::wrap(Box::new(move || {
            if let Some(tx) = open_tx.borrow_mut().take() {
                let _ = tx.send(true);
            }
        }) as Box<dyn FnMut()>);
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();

        let (err_tx, err_rx) = futures::channel::oneshot::channel::<()>();
        let err_tx = std::cell::RefCell::new(Some(err_tx));
        let onerror = Closure::wrap(Box::new(move || {
            if let Some(tx) = err_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn FnMut()>);
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();

        // Wait for either open or error
        let opened = tokio::select! {
            ok = open_rx => ok.unwrap_or(false),
            _ = err_rx => false,
        };

        if !opened {
            let _ = ws.close();
            gloo_timers::future::TimeoutFuture::new(backoff_ms).await;
            backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
            continue;
        }

        // Connected — reset backoff
        backoff_ms = 500;

        sender.set(Some(WebSocketSender { ws: ws.clone() }));

        // Listen for messages
        let (close_tx, close_rx) = futures::channel::oneshot::channel::<()>();
        let close_tx = std::cell::RefCell::new(Some(close_tx));

        let onmessage = {
            let latest = latest_event;
            Closure::wrap(Box::new(move |e: MessageEvent| {
                if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                    let s: String = text.into();
                    if let Ok(event) = serde_json::from_str::<ChatEvent>(&s) {
                        latest.set(Some(event));
                    }
                }
            }) as Box<dyn FnMut(MessageEvent)>)
        };
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();

        let onclose = Closure::wrap(Box::new(move || {
            if let Some(tx) = close_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn FnMut()>);
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onclose.forget();

        // Wait until closed
        let _ = close_rx.await;

        sender.set(None);

        // Reconnect with backoff
        gloo_timers::future::TimeoutFuture::new(backoff_ms).await;
        backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
    }
}
```

- [ ] **Step 2: Add WASM dependencies to `Cargo.toml`**

In the `[target.'cfg(target_arch = "wasm32")'.dependencies]` section, add:
```toml
web-sys = { version = "0.3", features = ["WebSocket", "BinaryType", "MessageEvent", "Location", "Window"] }
wasm-bindgen = "0.2"
js-sys = "0.3"
gloo-timers = { version = "0.3", features = ["futures"] }
futures = "0.3"
```

Also add `futures` to the non-wasm deps section (it's already being added in Task 2, this is a reminder).

- [ ] **Step 3: Add `pub mod use_websocket;` to `src/components/mod.rs`**

Add after the existing module declarations:
```rust
pub mod use_websocket;
```

- [ ] **Step 4: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles. Note: the `web_sys`/`wasm_bindgen` code is gated behind `#[cfg(target_arch = "wasm32")]` so it won't be compiled for the server target.

- [ ] **Step 5: Commit**

```bash
git add src/components/use_websocket.rs src/components/mod.rs Cargo.toml
git commit -m "feat(ws): add use_websocket client hook with auto-reconnect"
```

---

### Task 7: Initialize WebSocket in AuthLayout and wire up RoomView

**Files:**
- Modify: `src/components/auth_layout.rs` — init WS and provide as context
- Modify: `src/components/room_view.rs` — subscribe to room, append messages from WS
- Modify: `src/components/dm_view.rs` — subscribe to DM room, append messages from WS

- [ ] **Step 1: Modify `src/components/auth_layout.rs`**

Add the WebSocket hook initialization inside the `Some(Ok(Some(user)))` arm, after `use_context_provider(|| Signal::new(user.clone()))`:

```rust
use crate::components::use_websocket::use_websocket;

// Inside the Some(Ok(Some(user))) match arm:
let ws = use_websocket();
use_context_provider(|| ws);
```

The full arm becomes:
```rust
Some(Ok(Some(user))) => {
    use_context_provider(|| Signal::new(user.clone()));
    let ws = crate::components::use_websocket::use_websocket();
    use_context_provider(|| ws);
    rsx! {
        Outlet::<Route> {}
    }
}
```

- [ ] **Step 2: Modify `src/components/room_view.rs` to subscribe and handle WS events**

Add imports at top:
```rust
use crate::components::use_websocket::WsHandle;
use crate::ws::events::ChatEvent;
```

Inside `RoomViewPage`, after the `messages_version` signal, add:

```rust
let ws = use_context::<WsHandle>();

// Subscribe to this room's WS events
use_effect(move || {
    ws.subscribe(parsed_id);
    // Return cleanup that unsubscribes
    move || {
        ws.unsubscribe(parsed_id);
    }
});

// When a WS event arrives for this room, bump messages_version to trigger refetch
use_effect(move || {
    if let Some(ref event) = *ws.latest_event.read() {
        match event {
            ChatEvent::NewMessage { message } if message.room_id == parsed_id => {
                messages_version.set(messages_version() + 1);
            }
            ChatEvent::MessageDeleted { room_id, .. } if *room_id == parsed_id => {
                messages_version.set(messages_version() + 1);
            }
            _ => {}
        }
    }
});
```

Note: For the initial implementation, we bump `messages_version` to trigger a refetch via `use_server_future`. A future optimization could append messages directly to a local signal, but refetching is simpler and correct.

- [ ] **Step 3: Modify `src/components/dm_view.rs` to subscribe and handle WS events**

Add imports at top:
```rust
use crate::components::use_websocket::WsHandle;
use crate::ws::events::ChatEvent;
```

Inside `DmViewPage`, after `let room_id = room.id;` and the `messages_version` signal, add:

```rust
let ws = use_context::<WsHandle>();

// Subscribe to this DM room's WS events
use_effect(move || {
    ws.subscribe(room_id);
    move || {
        ws.unsubscribe(room_id);
    }
});

// When a WS event arrives for this room, bump messages_version
use_effect(move || {
    if let Some(ref event) = *ws.latest_event.read() {
        match event {
            ChatEvent::NewMessage { message } if message.room_id == room_id => {
                messages_version.set(messages_version() + 1);
            }
            _ => {}
        }
    }
});
```

- [ ] **Step 4: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles clean.

- [ ] **Step 5: Commit**

```bash
git add src/components/auth_layout.rs src/components/room_view.rs src/components/dm_view.rs
git commit -m "feat(ws): wire up WebSocket in auth layout, room view, and DM view"
```

---

### Task 8: Sidebar refresh on new DM messages

**Files:**
- Modify: `src/components/sidebar.rs`

- [ ] **Step 1: Modify `src/components/sidebar.rs` to react to WS events**

Add imports:
```rust
use crate::components::use_websocket::WsHandle;
use crate::ws::events::ChatEvent;
```

Add a `dms_version` signal (same pattern as room_view) and wire it to `use_server_future`:

Replace:
```rust
let dms = use_server_future(list_my_dms)?;
```

With:
```rust
let mut dms_version = use_signal(|| 0u32);
let dms = use_server_future(move || {
    let _v = dms_version();
    async move { list_my_dms().await }
})?;

let ws = use_context::<WsHandle>();

// Refresh DM list when any new DM message arrives
use_effect(move || {
    if let Some(ref event) = *ws.latest_event.read() {
        if matches!(event, ChatEvent::NewMessage { .. }) {
            dms_version.set(dms_version() + 1);
        }
    }
});
```

- [ ] **Step 2: Build check**

Run: `docker run --rm -v /home/nate/lets-chat:/app -w /app rust:1.93-slim-trixie cargo check 2>&1 | tail -20`
Expected: Compiles clean.

- [ ] **Step 3: Commit**

```bash
git add src/components/sidebar.rs
git commit -m "feat(ws): refresh sidebar DM list on WebSocket events"
```

---

## Notes

- **Ping/pong**: The browser's WebSocket API handles ping/pong automatically at the protocol level. The server-side axum WS also handles pongs automatically. No explicit ping task is needed for connection keepalive with modern browsers/axum, but stale connections will be detected when sends fail.
- **Desktop mode**: The desktop build also uses the embedded Axum server, so WebSocket works the same way — the WASM client connects to `ws://127.0.0.1:8080/ws`.
- **Security**: The WS endpoint validates the session cookie on connect. If a user is banned while connected, the `UserBanned` event is broadcast globally and the client can close/redirect. Session expiry while connected is acceptable — the next HTTP request will fail auth.
- **Scalability**: `broadcast::channel(64)` per connection means if a connection can't keep up with 64 buffered events, it will start losing events (lagged receiver). This is fine for a self-hosted chat app. The `DashMap` allows concurrent access from multiple Axum handler tasks without mutex contention.
