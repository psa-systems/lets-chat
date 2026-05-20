//! Web Push fan-out for `Mentioned` events.
//!
//! Public surface:
//! - `PushClient` trait: one method, `send`. Production wraps the
//!   `web-push-native` builder + `reqwest`; tests substitute
//!   `MockPushClient`.
//! - `dispatch`: the helper invoked from each `Mentioned`-broadcast
//!   site. Performs the mute/notify-push gating and spawns one
//!   fire-and-forget task per stored subscription.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::StatusCode;
use web_push_native::{
    jwt_simple::algorithms::ES256KeyPair, p256::PublicKey, Auth, WebPushBuilder,
};

use crate::db::{
    self, notifications::MuteMode, push_subscriptions::PushSubscription, vapid::VapidKeypair,
};
use crate::state::AppState;
use crate::ws::events::ChatEvent;

pub mod payload;

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("endpoint gone: {0}")]
    EndpointGone(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("encrypt: {0}")]
    Encrypt(String),
}

#[async_trait]
pub trait PushClient: Send + Sync {
    async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError>;
}

/// Production `PushClient`: builds an encrypted, VAPID-signed Web Push
/// request via `web-push-native` and sends it with the project's
/// existing `reqwest` (rustls) client. Holds the decrypted VAPID
/// keypair and the `mailto:` contact for the JWT `sub` claim.
pub struct ReqwestPushClient {
    http: reqwest::Client,
    vapid: Arc<VapidKeypair>,
    contact: String,
}

impl ReqwestPushClient {
    pub fn new(vapid: Arc<VapidKeypair>, contact: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            vapid,
            contact,
        }
    }
}

#[async_trait]
impl PushClient for ReqwestPushClient {
    async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

        let endpoint = sub
            .endpoint
            .parse::<http::Uri>()
            .map_err(|e| PushError::Encrypt(format!("endpoint uri: {e}")))?;
        let p256dh_bytes = URL_SAFE_NO_PAD
            .decode(&sub.p256dh_key)
            .map_err(|e| PushError::Encrypt(format!("p256dh b64: {e}")))?;
        let auth_bytes = URL_SAFE_NO_PAD
            .decode(&sub.auth_key)
            .map_err(|e| PushError::Encrypt(format!("auth b64: {e}")))?;
        if auth_bytes.len() != 16 {
            return Err(PushError::Encrypt(format!(
                "auth secret must be 16 bytes, got {}",
                auth_bytes.len()
            )));
        }
        let ua_public = PublicKey::from_sec1_bytes(&p256dh_bytes)
            .map_err(|e| PushError::Encrypt(format!("p256dh decode: {e}")))?;
        let ua_auth = Auth::clone_from_slice(&auth_bytes);

        let key_pair = ES256KeyPair::from_bytes(&self.vapid.private_key_bytes)
            .map_err(|e| PushError::Encrypt(format!("vapid keypair: {e}")))?;
        let request = WebPushBuilder::new(endpoint, ua_public, ua_auth)
            .with_vapid(&key_pair, &self.contact)
            .build(payload.to_vec())
            .map_err(|e| PushError::Encrypt(format!("encrypt: {e}")))?;

        let (parts, body) = request.into_parts();
        let mut req = self.http.post(parts.uri.to_string()).body(body);
        for (name, value) in parts.headers.iter() {
            if let Ok(v) = value.to_str() {
                req = req.header(name.as_str(), v);
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| PushError::Transport(e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status == StatusCode::GONE || status == StatusCode::NOT_FOUND {
            return Err(PushError::EndpointGone(sub.endpoint.clone()));
        }
        Err(PushError::Transport(format!("http {}", status.as_u16())))
    }
}

/// Test-only `PushClient`. Records every `send` for assertion.
#[derive(Default)]
pub struct MockPushClient {
    pub sent: tokio::sync::Mutex<Vec<RecordedSend>>,
}

#[derive(Debug, Clone)]
pub struct RecordedSend {
    pub endpoint: String,
    pub user_id: String,
    pub payload: Bytes,
}

#[async_trait]
impl PushClient for MockPushClient {
    async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError> {
        self.sent.lock().await.push(RecordedSend {
            endpoint: sub.endpoint.clone(),
            user_id: sub.user_id.clone(),
            payload,
        });
        Ok(())
    }
}

/// Process-global cap on the number of concurrent `client.send()` calls in
/// flight at any moment. Sized for a single-server deployment talking to
/// FCM / Mozilla autopush from one origin: well within typical per-server
/// rate caps, and enough headroom that an `@channel` in a large room
/// settles in a few seconds rather than serializing one push at a time.
/// If operator demand surfaces, this can become a setting; v1 hardcodes it.
pub const PUSH_FANOUT_CONCURRENCY: usize = 16;

/// Singleton Semaphore enforcing `PUSH_FANOUT_CONCURRENCY`. Lives at the
/// process scope: one cap per running server (or per integration-test
/// binary, since each `tests/*.rs` becomes its own binary). The permit is
/// acquired INSIDE each spawned send task so the cap binds the actual
/// network calls, not orchestration. This keeps `dispatch` fire-and-forget
/// so the HTTP handler that triggered the mention does not block waiting
/// for push-service round-trips.
fn push_fanout_sem() -> &'static tokio::sync::Semaphore {
    use std::sync::OnceLock;
    static SEM: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    SEM.get_or_init(|| tokio::sync::Semaphore::new(PUSH_FANOUT_CONCURRENCY))
}

/// Fan out a single `Mentioned`-equivalent Push to every registered
/// subscription for `recipient_user_id`. Honors:
///   1. global Push availability (`state.vapid` is some)
///   2. per-user `notify_push_enabled`
///   3. per-room mute mode (a row in `room_notification_settings` for the
///      `(recipient, room_id)` pair, which covers DM rooms uniformly since
///      a DM is a row in `rooms` with `room_type = 'dm'`)
///
/// Each subscription send runs as its own `tokio::spawn` task. The spawned
/// task acquires a permit from the process-global `push_fanout_sem` BEFORE
/// calling `client.send()`, so concurrent network calls across the entire
/// process are capped at `PUSH_FANOUT_CONCURRENCY`. Tasks that cannot
/// acquire a permit immediately await it; they never drop work.
///
/// Failures are logged at warn level. 410-Gone deletes the row inline.
///
/// This is fire-and-forget: `dispatch` returns once the spawn-loop is done
/// (microseconds). The HTTP handler that called it does not wait for any
/// push to settle; a user mentioning someone with three devices pays
/// roughly the same response time as mentioning someone with one device.
///
/// No "recall Push" path on edit: once a push is en-route to the device
/// vendor, the OS owns the notification. Same semantics as `@username`
/// today; documented here for future readers.
pub async fn dispatch(state: &AppState, recipient_user_id: &str, event: &ChatEvent) {
    if state.vapid.is_none() {
        return;
    }
    // LC-63: reminders also push. `mute_room` is the room to check against
    // the recipient's mute setting; `None` (reminders) skips the mute check,
    // since the user explicitly asked to be pinged.
    let mute_room = match event {
        ChatEvent::Mentioned { room_id, .. } => Some(*room_id),
        ChatEvent::Reminder { .. } => None,
        _ => return,
    };

    let recipient = match db::auth::find_user_by_id(&state.auth, recipient_user_id).await {
        Ok(Some(u)) => u,
        Ok(None) | Err(_) => return,
    };
    if !recipient.notify_push_enabled {
        return;
    }

    if let Some(room_id) = mute_room {
        let mode = db::notifications::room_mute_mode(&state.chat, recipient_user_id, room_id)
            .await
            .unwrap_or(MuteMode::None);
        if matches!(mode, MuteMode::All) {
            return;
        }
    }

    let subs = match db::push_subscriptions::for_user(&state.auth, recipient_user_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "push: subscription lookup failed");
            return;
        }
    };
    if subs.is_empty() {
        return;
    }

    let payload = match payload::build(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "push: payload build failed");
            return;
        }
    };
    for sub in subs {
        let client = state.push_client.clone();
        let auth_pool = state.auth.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            // Acquire here, not at the dispatch entry, so the cap binds
            // the actual network calls regardless of how many concurrent
            // dispatches are in flight. The semaphore is never closed;
            // the expect is unreachable in practice.
            let _permit = push_fanout_sem()
                .acquire()
                .await
                .expect("push fan-out semaphore not closed");
            match client.send(&sub, payload).await {
                Ok(()) => {
                    let _ = db::push_subscriptions::bump_last_seen(&auth_pool, &sub.endpoint).await;
                }
                Err(PushError::EndpointGone(_)) => {
                    let _ =
                        db::push_subscriptions::delete_by_endpoint(&auth_pool, &sub.endpoint).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, endpoint = %sub.endpoint, "push send failed");
                }
            }
        });
    }
}
