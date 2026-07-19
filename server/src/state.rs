use std::sync::Arc;

use sqlx::SqlitePool;

use crate::auth::LastSeenLedger;
use crate::bg::BgWriter;
use crate::db::vapid::VapidKeypair;
use crate::embeddings::EmbeddingClient;
use crate::llm::LlmClient;
use crate::mail::Mailer;
use crate::oidc::BunyipSsoClient;
use crate::push::{ApnsClient, FcmClient, PushClient};
use crate::rate_limit::RateLimits;
use crate::stt::SttClient;
use crate::ws::hub::Hub;

#[derive(Clone)]
pub struct AppState {
    pub auth: SqlitePool,
    pub chat: SqlitePool,
    pub settings: SqlitePool,
    pub hub: Arc<Hub>,
    pub asset_version: String,
    /// In-memory write-debounce ledger for `sessions.last_seen_at`. Shared
    /// across the auth middleware so a busy session does not write the
    /// column on every request.
    pub last_seen_ledger: LastSeenLedger,
    /// In-memory write-debounce ledger for `users.last_active_at`. Same
    /// shape as `last_seen_ledger`; collapses WS/room-visit storms into at
    /// most one write per user per debounce window.
    pub activity_ledger: LastSeenLedger,
    /// Background writer for high-frequency, low-value column updates
    /// (session last-seen, user activity). Handlers send a touch via the
    /// channel; one worker task batches them onto a single writer.
    pub bg: BgWriter,
    pub secret_key: Option<Arc<[u8; 32]>>,
    /// `Some` when `LETS_CHAT_SECRET_KEY` is set AND the VAPID keypair
    /// has been generated/loaded. `None` disables Push entirely (no
    /// subscribe route, no fan-out, settings checkbox shows disabled).
    pub vapid: Option<Arc<VapidKeypair>>,
    /// Always present. When `vapid` is `None`, the dispatch helper
    /// short-circuits before any client method is called.
    pub push_client: Arc<dyn PushClient>,
    /// LC-91: APNs (iOS) sender. `None` until the native client (LC-99/123)
    /// and operator APNs credentials exist; dispatch skips the channel when
    /// unconfigured. Device tokens are still recorded so delivery starts the
    /// moment a sender is wired.
    pub apns_client: Option<Arc<dyn ApnsClient>>,
    /// LC-91: FCM (Android) sender. `None` until configured; see `apns_client`.
    pub fcm_client: Option<Arc<dyn FcmClient>>,
    /// `Some` when `LETS_CHAT_SMTP_HOST` / `LETS_CHAT_SMTP_PORT` /
    /// `LETS_CHAT_SMTP_TLS` / `LETS_CHAT_SMTP_FROM` are set.
    /// `None` disables outbound mail; password reset routes return 404
    /// and the digest tick short-circuits.
    pub mailer: Option<Mailer>,
    /// LC-580: IP -> country resolver for login-location alerts. `None` when
    /// `IP2LOCATION_DB_PATH` is unset or the `.BIN` failed to load, which
    /// disables the alert (the login is never affected). `Arc` because
    /// `AppState` is `Clone` (axum state) and the resolver is not.
    pub geoip: Option<std::sync::Arc<crate::geoip::GeoipResolver>>,
    /// LC-587: opt-in suspicious-login notify-and-approve gate. When `true`
    /// (env `LOGIN_APPROVAL_ENABLED`), a login flagged as suspicious (new
    /// country and/or new device) at the Bunyip callback is withheld pending an
    /// emailed 6-digit code instead of minting a session. Default `false`
    /// leaves the LC-580 alert-only behaviour. Off can never withhold a login;
    /// on ships opt-in per deployment for a staged rollout (PMS-289 lesson).
    pub login_approval_enabled: bool,
    /// Absolute base URL used to build links inside outbound mail
    /// (password reset, email verification, and digest deep links).
    /// Defaults to `http://localhost:8080`.
    pub base_url: String,
    /// JSON array of `RTCIceServer` objects passed verbatim to the browser
    /// `RTCPeerConnection` for 1:1 WebRTC calls. Defaults to a public STUN
    /// server; override with `LETS_CHAT_ICE_SERVERS` to add a TURN server
    /// for NAT traversal when peers cannot connect directly.
    pub ice_servers: String,
    /// LC-94: per-process rate-limit counters. Shared via `Clone` so all
    /// handler tasks see the same DashMap.
    pub rate_limits: RateLimits,
    /// LC-22 pure-RP cutover: the Bunyip SSO client. Mandatory at startup
    /// in production: `main.rs` panics when the four `LETS_CHAT_BUNYIP_SSO_*`
    /// env vars are missing or bunyip-api is unreachable for discovery.
    ///
    /// Typed `Option<...>` for tests: integration test binaries construct
    /// `AppState` by hand and do not exercise the SSO callback, so `None`
    /// keeps them building without standing up a mock OP. Route handlers
    /// `.expect("bunyip_sso required")` because production guarantees it.
    /// See `docs/lets-chat/sso/bunyip-only/04-lets-chat-server-cutover.md`
    /// §4.2.
    pub bunyip_sso: Option<Arc<BunyipSsoClient>>,
    /// LC-393 Phase 3: server-side STT for call transcription. `Some` when
    /// `LETS_CHAT_STT_URL` is set; the browser then captures audio clips and
    /// the `/call/transcript/{id}/audio` route forwards them here. `None`
    /// (default) keeps transcription on the in-browser Web Speech engine.
    pub stt_client: Option<Arc<dyn SttClient>>,
    /// LC-396: optional LLM for transcript summaries. `Some` when
    /// `LETS_CHAT_LLM_URL` is set; gates the "Summarize" action on the
    /// transcript page. `None` (default) hides it.
    pub llm_client: Option<Arc<dyn LlmClient>>,
    /// LC-549: optional text-embeddings endpoint for semantic / related-message
    /// search. `Some` when `LETS_CHAT_EMBEDDINGS_URL` is set; gates the
    /// per-message "Find related" action and the opt-in semantic search mode,
    /// and drives background embedding of new messages. `None` (default) hides
    /// the features and search stays on the FTS keyword path.
    pub embedding_client: Option<Arc<dyn EmbeddingClient>>,
}

impl AppState {
    /// True when the VAPID keypair has been initialized and Web Push
    /// fan-out / subscription routes are operational.
    pub fn push_available(&self) -> bool {
        self.vapid.is_some()
    }
    /// True when an SMTP mailer has been configured. Password reset,
    /// email verification, and email digest are off-limits without one.
    pub fn mail_available(&self) -> bool {
        self.mailer.is_some()
    }
    /// LC-393 Phase 3: true when a server-side STT endpoint is configured, so
    /// clients use the server engine (audio upload) instead of the browser one.
    pub fn stt_available(&self) -> bool {
        self.stt_client.is_some()
    }
    /// LC-396: true when an LLM endpoint is configured (transcript summaries).
    pub fn llm_available(&self) -> bool {
        self.llm_client.is_some()
    }
    /// LC-549: true when a text-embeddings endpoint is configured, so the
    /// "Find related" action and semantic search mode are available (search
    /// otherwise degrades to FTS keyword matching).
    pub fn embeddings_available(&self) -> bool {
        self.embedding_client.is_some()
    }
    /// Whether session and pending-auth cookies should carry the `Secure`
    /// attribute. WebKit2GTK (Tauri desktop on Linux) and Safari reject
    /// `Secure` cookies on `http://` URLs, including `http://localhost`,
    /// which silently breaks login on dev setups served over plain HTTP.
    /// We tie the flag to `base_url`'s scheme: if the operator configured
    /// an HTTPS base URL the client is reaching us over TLS (directly or
    /// through a terminating proxy), so Secure is correct. For the default
    /// `http://localhost:8080` dev base URL, Secure is dropped so cookies
    /// stick.
    pub fn cookies_secure(&self) -> bool {
        self.base_url.starts_with("https://")
    }
    /// LC-22: the configured Bunyip RP client. Panics in tests that try to
    /// reach the SSO surface without standing one up; production guarantees
    /// Some via main.rs startup.
    pub fn bunyip_sso_client(&self) -> &Arc<BunyipSsoClient> {
        self.bunyip_sso
            .as_ref()
            .expect("bunyip_sso required; main.rs panics when env is missing")
    }
}
