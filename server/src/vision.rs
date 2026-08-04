//! LC-667: optional operator VISION endpoint for auto-drafting image alt text.
//!
//! Separate from the text LLM (`crate::llm`): describing an image needs a
//! multimodal model (llava, llama3.2-vision, ...), which is heavier and usually
//! a different endpoint than the chat LLM. Configured via `LETS_CHAT_VISION_*`;
//! when unset the "Auto-draft" affordance is hidden (CSS-gated on the
//! `[data-lc-vision]` page root) and the alt-draft route 400s. Same
//! operator-trusted, NOT-SSRF-filtered posture as the LLM/STT clients (a
//! localhost multimodal server works), and the image never leaves the box.

use async_trait::async_trait;

/// Cap on the drafted alt text (the alt attribute is bounded on write anyway).
const MAX_ALT_DRAFT_CHARS: usize = 600;

/// The alt-text prompt: one concise, screen-reader-friendly sentence.
const DRAFT_PROMPT: &str = "You write concise alt text for an image in a team chat app, for people \
using screen readers. Describe the image in ONE plain sentence (roughly 5 to 20 words) covering what \
matters. Do not begin with \"image of\" or \"picture of\", do not add quotation marks, and do not \
guess at text you cannot clearly read. Output only the description.";

/// Operator vision configuration. `None` from [`VisionConfig::from_env`] means
/// image alt-text drafting is disabled and the UI hides the action.
#[derive(Debug, Clone)]
pub struct VisionConfig {
    /// Full chat-completions endpoint (OpenAI vision shape).
    pub url: String,
    pub api_key: Option<String>,
    /// Model name sent in the request. Defaults to `llava`.
    pub model: String,
}

impl VisionConfig {
    /// `LETS_CHAT_VISION_URL` (required to enable), `LETS_CHAT_VISION_API_KEY`
    /// (optional), `LETS_CHAT_VISION_MODEL` (optional, default `llava`).
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("LETS_CHAT_VISION_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let api_key = std::env::var("LETS_CHAT_VISION_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let model = std::env::var("LETS_CHAT_VISION_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "llava".to_string());
        Some(Self {
            url,
            api_key,
            model,
        })
    }
}

/// Whether an operator vision endpoint is configured. Drives the
/// `[data-lc-vision]` page-root gate and the alt-draft route. Reads the env each
/// call; the alt-draft path is a rare user click, not a hot loop.
pub fn available() -> bool {
    VisionConfig::from_env().is_some()
}

#[derive(Debug)]
pub enum VisionError {
    Transport(String),
    BadResponse(String),
}

impl std::fmt::Display for VisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisionError::Transport(e) => write!(f, "vision transport error: {e}"),
            VisionError::BadResponse(e) => write!(f, "vision bad response: {e}"),
        }
    }
}

/// An operator vision endpoint. `describe` turns an image into a one-line alt
/// text draft. Mockable for tests.
#[async_trait]
pub trait VisionClient: Send + Sync {
    /// Describe the image at `data_url` (a `data:<mime>;base64,...` URL) as alt
    /// text. The prompt is the client's concern, so callers pass only the image.
    async fn describe(&self, data_url: &str) -> Result<String, VisionError>;
}

/// Production client: OpenAI-vision-shaped chat-completions POST to the operator
/// endpoint via the blessed trusted-outbound helper.
pub struct ReqwestVisionClient {
    cfg: VisionConfig,
}

impl ReqwestVisionClient {
    pub fn new(cfg: VisionConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl VisionClient for ReqwestVisionClient {
    async fn describe(&self, data_url: &str) -> Result<String, VisionError> {
        let payload = serde_json::json!({
            "model": self.cfg.model,
            "temperature": 0.2,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": DRAFT_PROMPT },
                    { "type": "image_url", "image_url": { "url": data_url } },
                ],
            }],
        });
        let mut req = crate::http_client::outbound_trusted_post(&self.cfg.url)
            .await
            .map_err(|e| VisionError::Transport(e.to_string()))?
            .json(&payload)
            // Vision inference on a CPU/GPU-bound local model can be slow.
            .timeout(std::time::Duration::from_secs(120));
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| VisionError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(VisionError::BadResponse(format!(
                "status {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VisionError::BadResponse(e.to_string()))?;
        let text = body
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(VisionError::BadResponse("empty description".to_string()));
        }
        Ok(text.chars().take(MAX_ALT_DRAFT_CHARS).collect())
    }
}

/// Test double returning a canned description.
pub struct MockVisionClient {
    pub canned: String,
}

#[async_trait]
impl VisionClient for MockVisionClient {
    async fn describe(&self, _data_url: &str) -> Result<String, VisionError> {
        Ok(self.canned.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_prompt_asks_for_concise_screen_reader_alt_text() {
        let lo = DRAFT_PROMPT.to_lowercase();
        assert!(lo.contains("alt text"));
        assert!(lo.contains("one plain sentence") || lo.contains("one sentence"));
        // Guards the two classic alt-text anti-patterns.
        assert!(lo.contains("image of"));
        assert!(lo.contains("do not guess"));
    }

    #[tokio::test]
    async fn mock_client_returns_its_canned_description() {
        let c = MockVisionClient {
            canned: "a green off-road truck parked on grass".into(),
        };
        assert_eq!(
            c.describe("data:image/png;base64,AAAA").await.unwrap(),
            "a green off-road truck parked on grass"
        );
    }
}
