use std::sync::Arc;

use sqlx::SqlitePool;

use crate::ws::hub::Hub;

#[derive(Clone)]
pub struct AppState {
    pub auth: SqlitePool,
    pub chat: SqlitePool,
    pub settings: SqlitePool,
    pub hub: Arc<Hub>,
    pub asset_version: String,
    pub secret_key: Option<Arc<[u8; 32]>>,
}

impl AppState {
    /// True when a stable encryption key is configured. 2FA flows are
    /// off-limits without one.
    pub fn two_factor_available(&self) -> bool {
        self.secret_key.is_some()
    }
}
