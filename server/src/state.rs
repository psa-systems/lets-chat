use std::sync::Arc;

use sqlx::SqlitePool;

use crate::db::vapid::VapidKeypair;
use crate::push::PushClient;
use crate::ws::hub::Hub;

#[derive(Clone)]
pub struct AppState {
    pub auth: SqlitePool,
    pub chat: SqlitePool,
    pub settings: SqlitePool,
    pub hub: Arc<Hub>,
    pub asset_version: String,
    pub secret_key: Option<Arc<[u8; 32]>>,
    /// `Some` when `LETS_CHAT_SECRET_KEY` is set AND the VAPID keypair
    /// has been generated/loaded. `None` disables Push entirely (no
    /// subscribe route, no fan-out, settings checkbox shows disabled).
    pub vapid: Option<Arc<VapidKeypair>>,
    /// Always present. When `vapid` is `None`, the dispatch helper
    /// short-circuits before any client method is called.
    pub push_client: Arc<dyn PushClient>,
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
}
