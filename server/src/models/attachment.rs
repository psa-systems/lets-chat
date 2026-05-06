use serde::{Deserialize, Serialize};

/// Public-facing attachment view-model. Templates render this directly; the
/// underlying `storage_path` from `file_uploads` never leaves the DB layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    /// Client-facing fetch URL: `/api/files/{id}`.
    pub url: String,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }
}
