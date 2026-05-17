use std::sync::Arc;

use sqlx::SqlitePool;

use crate::auth::LastSeenLedger;
use crate::bg::BgWriter;
use crate::db::vapid::VapidKeypair;
use crate::mail::Mailer;
use crate::push::PushClient;
use crate::sso::SsoProviders;
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
    /// `Some` when `LETS_CHAT_SMTP_URL` + `LETS_CHAT_SMTP_FROM` are set.
    /// `None` disables outbound mail; password reset routes return 404
    /// and the digest tick short-circuits.
    pub mailer: Option<Mailer>,
    /// Absolute base URL used to build links inside outbound mail
    /// (password reset, email verification, and digest deep links).
    /// Defaults to `http://localhost:8080`.
    pub base_url: String,
    /// JSON array of `RTCIceServer` objects passed verbatim to the browser
    /// `RTCPeerConnection` for 1:1 WebRTC calls. Defaults to a public STUN
    /// server; override with `LETS_CHAT_ICE_SERVERS` to add a TURN server
    /// for NAT traversal when peers cannot connect directly.
    pub ice_servers: String,
    /// Configured SSO providers. Empty when SSO is not configured for
    /// this deployment (the existing username/password flow is the
    /// only auth path). v1 holds at most one entry under the "default"
    /// provider id; multi-IdP (BYO-SSO) extends this without a
    /// schema/state change. See `crate::sso`.
    pub sso: SsoProviders,
    /// True when `LETS_CHAT_LOCAL_LOGIN_DISABLED` is set. Per doc 10
    /// section 8 this is a hard kill switch for the password sign-in
    /// path: POST /login, /register, /forgot, /reset/{token},
    /// /settings/password, and /verify-email/resend all 404. Existing
    /// users keep their `password_hash` rows; nothing is dropped from
    /// the DB. The login page UI hides the password form (L21).
    /// Per-user lockdown is a separate follow-up.
    pub local_login_disabled: bool,
}

impl AppState {
    /// True when a stable encryption key is configured. 2FA flows are
    /// off-limits without one.
    pub fn two_factor_available(&self) -> bool {
        self.secret_key.is_some()
    }
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
    /// True when at least one SSO provider is configured. Login page
    /// renders the "Sign in with SSO" button conditionally on this.
    pub async fn sso_available(&self) -> bool {
        !self.sso.is_empty().await
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
}
