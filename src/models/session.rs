/// Session record. Server-side only.
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}
