use std::sync::Arc;

use sqlx::SqlitePool;

use crate::ws::hub::Hub;

#[derive(Clone)]
pub struct AppState {
    pub auth: SqlitePool,
    pub chat: SqlitePool,
    pub settings: SqlitePool,
    pub hub: Arc<Hub>,
    pub asset_version: &'static str,
}

impl AppState {
    pub fn asset_url(&self, path: &str) -> String {
        format!(
            "/assets/{}?v={}",
            path.trim_start_matches('/'),
            self.asset_version
        )
    }
}
