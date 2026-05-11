/// Stored row for an enclave-scoped custom emoji. The file itself lives in
/// the shared `uploads_dir()` (content-addressed by sha256), so multiple
/// emojis pointing at the same image share storage transparently.
#[derive(Debug, Clone)]
pub struct CustomEmoji {
    pub id: i64,
    pub enclave_id: i64,
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
