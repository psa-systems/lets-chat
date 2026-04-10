use dioxus::prelude::*;

use crate::ws::events::{ChatEvent, ClientControl};

/// Provides a reactive stream of ChatEvent from the server WebSocket.
#[derive(Clone)]
pub struct WsHandle {
    pub latest_event: Signal<Option<ChatEvent>>,
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = msg;
        }
    }
}

/// Initialize WebSocket connection. Call once in AuthLayout.
pub fn use_websocket() -> WsHandle {
    let latest_event = use_signal(|| None::<ChatEvent>);
    let sender = use_signal(|| None::<WebSocketSender>);

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
    latest_event: Signal<Option<ChatEvent>>,
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
        let opened = futures::future::select(
            Box::pin(open_rx),
            Box::pin(err_rx),
        )
        .await;

        let connected = match opened {
            futures::future::Either::Left((Ok(true), _)) => true,
            _ => false,
        };

        if !connected {
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
            let mut latest = latest_event;
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
