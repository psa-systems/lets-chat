/// Stored row for a custom emoji. The file itself lives in the shared
/// `uploads_dir()` (content-addressed by sha256), so multiple emojis pointing
/// at the same image share storage transparently. LC-482: a row is either
/// enclave-scoped (`enclave_id` set) or user-scoped/personal (`user_id` set);
/// exactly one is non-NULL (enforced by a CHECK in migration 0075).
#[derive(Debug, Clone)]
pub struct CustomEmoji {
    pub id: i64,
    pub enclave_id: Option<i64>,
    /// LC-482: owner of a personal emoji; None for enclave-scoped rows.
    pub user_id: Option<String>,
    pub shortcode: String,
    pub storage_path: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploaded_by: String,
    pub created_at: String,
}

/// Cheap, render-only projection used by the body/reaction renderer to swap
/// `:shortcode:` tokens for `<img src="/api/emojis/{id}">`. The renderer
/// builds a `HashMap<&str, &EmojiRef>` from a slice of these per render pass.
#[derive(Debug, Clone)]
pub struct EmojiRef {
    pub id: i64,
    pub shortcode: String,
}
