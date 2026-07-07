//! LC-549: optional text-embeddings client for semantic / related-message search.
//!
//! Same operator-config posture as the LLM client (`crate::llm`, LC-396): when an
//! operator points `LETS_CHAT_EMBEDDINGS_URL` at an OpenAI-compatible
//! `/v1/embeddings` endpoint, new messages are embedded in the background and two
//! features light up: a per-message "Find related" action and an opt-in semantic
//! mode on room search. When no endpoint is configured the client is `None`, the
//! features are hidden, and search degrades to the existing FTS keyword path.
//!
//! Reached through `http_client::outbound_trusted_post` (NOT public-IP
//! SSRF-filtered) so a localhost engine (llama.cpp / Ollama / a local embedder)
//! works. Mockable so the ranking paths are testable without a live model.
//!
//! Vectors are stored as little-endian `f32` BLOBs in the `message_embeddings`
//! sidecar table (see `db::message_embeddings`); ranking is a cosine nearest-
//! neighbour scan done in Rust, which is fine at self-host scale.

use async_trait::async_trait;

/// Operator embeddings configuration. `None` from [`EmbeddingsConfig::from_env`]
/// means embeddings are disabled and the UI hides the related/semantic actions.
#[derive(Debug, Clone)]
pub struct EmbeddingsConfig {
    /// Full embeddings endpoint (e.g. `http://127.0.0.1:11434/v1/embeddings`).
    pub url: String,
    pub api_key: Option<String>,
    /// Model name sent in the request. Defaults to `text-embedding-3-small`.
    pub model: String,
}

impl EmbeddingsConfig {
    /// `LETS_CHAT_EMBEDDINGS_URL` (required to enable),
    /// `LETS_CHAT_EMBEDDINGS_API_KEY` (optional), `LETS_CHAT_EMBEDDINGS_MODEL`
    /// (optional, default `text-embedding-3-small`).
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("LETS_CHAT_EMBEDDINGS_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let api_key = std::env::var("LETS_CHAT_EMBEDDINGS_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let model = std::env::var("LETS_CHAT_EMBEDDINGS_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "text-embedding-3-small".to_string());
        Some(Self {
            url,
            api_key,
            model,
        })
    }
}

#[derive(Debug)]
pub enum EmbeddingError {
    Transport(String),
    BadResponse(String),
}

impl std::fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbeddingError::Transport(e) => write!(f, "embeddings transport error: {e}"),
            EmbeddingError::BadResponse(e) => write!(f, "embeddings bad response: {e}"),
        }
    }
}

/// An operator embeddings endpoint. `embed` returns one vector for the given
/// text. Mockable for tests.
#[async_trait]
pub trait EmbeddingClient: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
}

/// Production client: `/v1/embeddings` POST to the operator's endpoint via the
/// blessed trusted-outbound helper (see the module note).
pub struct ReqwestEmbeddingClient {
    cfg: EmbeddingsConfig,
}

impl ReqwestEmbeddingClient {
    pub fn new(cfg: EmbeddingsConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl EmbeddingClient for ReqwestEmbeddingClient {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let payload = serde_json::json!({
            "model": self.cfg.model,
            "input": text,
        });
        let mut req = crate::http_client::outbound_trusted_post(&self.cfg.url)
            .await
            .map_err(|e| EmbeddingError::Transport(e.to_string()))?
            .json(&payload)
            .timeout(std::time::Duration::from_secs(30));
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| EmbeddingError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(EmbeddingError::BadResponse(format!(
                "status {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EmbeddingError::BadResponse(e.to_string()))?;
        let arr = body
            .pointer("/data/0/embedding")
            .and_then(|v| v.as_array())
            .ok_or_else(|| EmbeddingError::BadResponse("no embedding in response".to_string()))?;
        let vec: Vec<f32> = arr
            .iter()
            .filter_map(|n| n.as_f64().map(|f| f as f32))
            .collect();
        if vec.is_empty() {
            return Err(EmbeddingError::BadResponse("empty embedding".to_string()));
        }
        Ok(vec)
    }
}

/// Cosine similarity in `[-1, 1]`. Returns `0.0` for a length mismatch or a
/// zero-norm vector (so an all-zero embedding never ranks above a real match).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Pack a vector into a little-endian `f32` byte BLOB for storage.
pub fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Unpack a little-endian `f32` byte BLOB back into a vector. A trailing partial
/// chunk (corrupt row) is ignored rather than panicking.
pub fn bytes_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Test double: a deterministic bag-of-words hashed embedding, so texts that
/// share words land on the same dimensions and rank as similar. Lets the ranking
/// paths be tested without a live model.
pub struct MockEmbeddingClient {
    pub dim: usize,
}

impl Default for MockEmbeddingClient {
    fn default() -> Self {
        Self { dim: 64 }
    }
}

/// Deterministic hashed bag-of-words vector. Shared words -> shared dimensions
/// -> positive cosine. Used only by the mock and its tests.
pub fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        // FNV-1a over the word bytes -> a stable bucket.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in word.bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let idx = (h % dim as u64) as usize;
        v[idx] += 1.0;
    }
    v
}

#[async_trait]
impl EmbeddingClient for MockEmbeddingClient {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(hash_embed(text, self.dim))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basic_properties() {
        let a = [1.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        let c = [0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
        // Length mismatch and zero-norm are safe zeros.
        assert_eq!(cosine_similarity(&a, &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&a, &[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn bytes_round_trip() {
        let v = vec![0.5f32, -1.25, 3.0, 0.0];
        assert_eq!(bytes_to_vec(&vec_to_bytes(&v)), v);
        // Trailing partial chunk is dropped, not panicked on.
        let mut bytes = vec_to_bytes(&v);
        bytes.push(0xAB);
        assert_eq!(bytes_to_vec(&bytes), v);
    }

    #[test]
    fn hash_embed_ranks_shared_words_higher() {
        let q = hash_embed("database migration failed", 64);
        let near = hash_embed("the database migration is broken", 64);
        let far = hash_embed("lunch plans for friday afternoon", 64);
        assert!(
            cosine_similarity(&q, &near) > cosine_similarity(&q, &far),
            "shared-word text should rank above unrelated text"
        );
    }
}
