//! LC-396: optional LLM summarization for saved call transcripts.
//!
//! When an operator configures an LLM endpoint, the transcript page offers a
//! "Summarize" action that sends the transcript text to an OpenAI-compatible
//! `/v1/chat/completions` endpoint and stores the returned summary + action
//! items. Self-hostable (point at a local llama.cpp / Ollama / vLLM server) and
//! keeps transcript content off third-party clouds unless the operator opts in.
//!
//! Same trust + plumbing posture as the Phase 3 STT client (`crate::stt`): the
//! endpoint is operator-configured and reached through
//! `http_client::outbound_trusted_post` (NOT public-IP SSRF-filtered, so a
//! localhost engine works). Mockable so the summary route is testable without a
//! live model.

use async_trait::async_trait;

/// Operator LLM configuration. `None` from [`LlmConfig::from_env`] means
/// summarization is disabled and the UI hides the action.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Full chat-completions endpoint (e.g. `http://127.0.0.1:11434/v1/chat/completions`).
    pub url: String,
    pub api_key: Option<String>,
    /// Model name sent in the request. Defaults to `gpt-4o-mini`.
    pub model: String,
}

impl LlmConfig {
    /// `LETS_CHAT_LLM_URL` (required to enable), `LETS_CHAT_LLM_API_KEY`
    /// (optional), `LETS_CHAT_LLM_MODEL` (optional, default `gpt-4o-mini`).
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("LETS_CHAT_LLM_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let api_key = std::env::var("LETS_CHAT_LLM_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let model = std::env::var("LETS_CHAT_LLM_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        Some(Self {
            url,
            api_key,
            model,
        })
    }
}

#[derive(Debug)]
pub enum LlmError {
    Transport(String),
    BadResponse(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Transport(e) => write!(f, "llm transport error: {e}"),
            LlmError::BadResponse(e) => write!(f, "llm bad response: {e}"),
        }
    }
}

/// The system prompt: ask for a concise summary + bulleted action items, in
/// markdown (rendered through the message markdown pipeline on display).
const SYSTEM_PROMPT: &str = "You summarize meeting and call transcripts. Reply in \
GitHub-flavored markdown with a short '## Summary' section (2-4 sentences) followed \
by a '## Action items' section as a bullet list (write 'None.' if there are none). \
Be concise and faithful to the transcript; do not invent details.";

/// Summarize a transcript -> markdown summary. Mockable for tests.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn summarize(&self, transcript: &str) -> Result<String, LlmError>;
}

/// Production client: chat-completions POST to the operator's endpoint via the
/// blessed trusted-outbound helper (see the module note).
pub struct ReqwestLlmClient {
    cfg: LlmConfig,
}

impl ReqwestLlmClient {
    pub fn new(cfg: LlmConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl LlmClient for ReqwestLlmClient {
    async fn summarize(&self, transcript: &str) -> Result<String, LlmError> {
        let payload = serde_json::json!({
            "model": self.cfg.model,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": transcript },
            ],
        });
        let mut req = crate::http_client::outbound_trusted_post(&self.cfg.url)
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?
            .json(&payload)
            // Summaries can take a while on a CPU model; give it room.
            .timeout(std::time::Duration::from_secs(120));
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(LlmError::BadResponse(format!("status {}", resp.status())));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))?;
        let text = body
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(LlmError::BadResponse("empty completion".to_string()));
        }
        Ok(text)
    }
}

/// Test double returning a canned summary.
pub struct MockLlmClient {
    pub canned: String,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn summarize(&self, _transcript: &str) -> Result<String, LlmError> {
        Ok(self.canned.clone())
    }
}
