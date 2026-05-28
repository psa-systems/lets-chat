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
    self, apns_subscriptions::ApnsSubscription, fcm_subscriptions::FcmSubscription,
    notifications::MuteMode, push_subscriptions::PushSubscription, vapid::VapidKeypair,
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
/// request via `web-push-native` and sends it through the LC-152 shared
/// outbound helper (`http_client::outbound_post`). Holds the decrypted
/// VAPID keypair and the `mailto:` contact for the JWT `sub` claim.
///
/// **LC-152 security fix.** This path previously constructed a raw
/// `reqwest::Client::new()` with no SSRF guard. The destination is a
/// user-supplied `PushSubscription.endpoint` (whatever URL the browser
/// registered when subscribing); a malicious user could register a
/// subscription pointing at e.g. `http://169.254.169.254/latest/...` (AWS
/// metadata) or an internal admin port, and lets-chat would POST
/// notifications there on every mention. Routing every request through
/// `outbound_post` puts the two-layer SSRF guard (URL-input filter for
/// literal IPs + `PublicOnlyResolver` for hostname TOCTOU) on this path
/// by construction. Public push gateways (Firefox / Chrome / Safari)
/// resolve public and pass through unaffected.
///
/// Does NOT hold a `reqwest::Client` field by design: the only way to
/// reach a client is through the helper's URL-validating `outbound_*`
/// entry points, so each `.send()` re-routes through the guard. If a
/// future requirement legitimately needs per-instance client tuning, the
/// answer is more `outbound_*` variants in `http_client`, NOT a
/// constructor parameter here.
pub struct ReqwestPushClient {
    vapid: Arc<VapidKeypair>,
    contact: String,
}

impl ReqwestPushClient {
    pub fn new(vapid: Arc<VapidKeypair>, contact: String) -> Self {
        Self { vapid, contact }
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
        // LC-152: every Web Push delivery routes through
        // `http_client::outbound_post`, which applies the two-layer SSRF
        // guard. A user-controlled `sub.endpoint` pointing at e.g.
        // `http://169.254.169.254/...` (AWS metadata) is refused by the
        // URL-input filter before any TCP connect.
        let url_string = parts.uri.to_string();
        let mut req = crate::http_client::outbound_post(&url_string)
            .await
            .map_err(|e| PushError::Transport(format!("ssrf-rejected: {e}")))?
            .body(body);
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

// LC-91: native mobile push channels. Each is a thin trait so the dispatch
// fan-out stays channel-agnostic; the production HTTP senders (APNs
// token-based JWT, FCM HTTP v1) land when the native client (LC-99/LC-123)
// and operator credentials exist. `AppState` carries each as an `Option`, so
// an unconfigured channel is simply skipped at dispatch time.

#[async_trait]
pub trait ApnsClient: Send + Sync {
    /// Deliver `payload` to one iOS device token. A dead token (APNs
    /// `BadDeviceToken` / `Unregistered`) must surface as
    /// `PushError::EndpointGone(device_token)` so the dispatch path prunes it.
    async fn send(&self, sub: &ApnsSubscription, payload: Bytes) -> Result<(), PushError>;
}

#[async_trait]
pub trait FcmClient: Send + Sync {
    /// Deliver `payload` to one Android registration token. A dead token (FCM
    /// `NOT_REGISTERED` / `UNREGISTERED`) must surface as
    /// `PushError::EndpointGone(registration_token)` so it is pruned.
    async fn send(&self, sub: &FcmSubscription, payload: Bytes) -> Result<(), PushError>;
}

/// One recorded mobile send (APNs or FCM), keyed by the device/registration
/// token. Mirrors `RecordedSend` for the Web Push mock.
#[derive(Debug, Clone)]
pub struct RecordedMobileSend {
    pub token: String,
    pub user_id: String,
    pub payload: Bytes,
}

/// Test-only `ApnsClient`. Records every `send` for assertion.
#[derive(Default)]
pub struct MockApnsClient {
    pub sent: tokio::sync::Mutex<Vec<RecordedMobileSend>>,
}

#[async_trait]
impl ApnsClient for MockApnsClient {
    async fn send(&self, sub: &ApnsSubscription, payload: Bytes) -> Result<(), PushError> {
        self.sent.lock().await.push(RecordedMobileSend {
            token: sub.device_token.clone(),
            user_id: sub.user_id.clone(),
            payload,
        });
        Ok(())
    }
}

/// Test-only `FcmClient`. Records every `send` for assertion.
#[derive(Default)]
pub struct MockFcmClient {
    pub sent: tokio::sync::Mutex<Vec<RecordedMobileSend>>,
}

#[async_trait]
impl FcmClient for MockFcmClient {
    async fn send(&self, sub: &FcmSubscription, payload: Bytes) -> Result<(), PushError> {
        self.sent.lock().await.push(RecordedMobileSend {
            token: sub.registration_token.clone(),
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
    // LC-91: bail cheap if no channel is configured at all. Web Push needs
    // VAPID; the mobile channels each carry their own client (`None` until the
    // live senders + operator credentials land).
    if state.vapid.is_none() && state.apns_client.is_none() && state.fcm_client.is_none() {
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

    // LC-88: Do Not Disturb. A push is a real-time toast; there is no point
    // delivering one that arrives during the user's quiet hours, so we drop it
    // (the in-app activity record was already written upstream). The email
    // digest, by contrast, holds and re-sends; see digest::run_tick. LC-91:
    // this and the mute check below gate every channel uniformly, since they
    // run before any per-channel fan-out.
    if crate::dnd::is_suppressed(&recipient, chrono::Utc::now()) {
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

    // One payload shape for every channel (AC: consistent deep-link / title /
    // body across Web Push, APNs, FCM). Built once and cloned per send.
    let payload = match payload::build(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "push: payload build failed");
            return;
        }
    };

    fan_out_webpush(state, recipient_user_id, &payload).await;
    fan_out_apns(state, recipient_user_id, &payload).await;
    fan_out_fcm(state, recipient_user_id, &payload).await;
}

/// Web Push fan-out: one spawned task per stored subscription. No-op when
/// VAPID is unconfigured.
async fn fan_out_webpush(state: &AppState, recipient_user_id: &str, payload: &Bytes) {
    if state.vapid.is_none() {
        return;
    }
    let subs = match db::push_subscriptions::for_user(&state.auth, recipient_user_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "push: webpush subscription lookup failed");
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

/// APNs (iOS) fan-out. No-op when no APNs client is configured. A dead token
/// (`EndpointGone`) is pruned inline, mirroring the Web Push 410 path.
async fn fan_out_apns(state: &AppState, recipient_user_id: &str, payload: &Bytes) {
    let Some(client) = state.apns_client.clone() else {
        return;
    };
    let subs = match db::apns_subscriptions::for_user(&state.auth, recipient_user_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "push: apns subscription lookup failed");
            return;
        }
    };
    for sub in subs {
        let client = client.clone();
        let auth_pool = state.auth.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            let _permit = push_fanout_sem()
                .acquire()
                .await
                .expect("push fan-out semaphore not closed");
            match client.send(&sub, payload).await {
                Ok(()) => {
                    let _ =
                        db::apns_subscriptions::bump_last_seen(&auth_pool, &sub.device_token).await;
                }
                Err(PushError::EndpointGone(_)) => {
                    let _ = db::apns_subscriptions::delete_by_token(&auth_pool, &sub.device_token)
                        .await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, token = %sub.device_token, "apns send failed");
                }
            }
        });
    }
}

/// FCM (Android) fan-out. No-op when no FCM client is configured. A dead token
/// (`EndpointGone`) is pruned inline.
async fn fan_out_fcm(state: &AppState, recipient_user_id: &str, payload: &Bytes) {
    let Some(client) = state.fcm_client.clone() else {
        return;
    };
    let subs = match db::fcm_subscriptions::for_user(&state.auth, recipient_user_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "push: fcm subscription lookup failed");
            return;
        }
    };
    for sub in subs {
        let client = client.clone();
        let auth_pool = state.auth.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            let _permit = push_fanout_sem()
                .acquire()
                .await
                .expect("push fan-out semaphore not closed");
            match client.send(&sub, payload).await {
                Ok(()) => {
                    let _ =
                        db::fcm_subscriptions::bump_last_seen(&auth_pool, &sub.registration_token)
                            .await;
                }
                Err(PushError::EndpointGone(_)) => {
                    let _ =
                        db::fcm_subscriptions::delete_by_token(&auth_pool, &sub.registration_token)
                            .await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, token = %sub.registration_token, "fcm send failed");
                }
            }
        });
    }
}
