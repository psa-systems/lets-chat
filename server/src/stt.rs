//! LC-393 Phase 3: optional server-side speech-to-text for call transcription.
//!
//! Phase 1/2 transcribe in the browser (Web Speech API). Phase 3 lets an
//! operator point lets-chat at a self-hosted STT endpoint instead: the browser
//! captures short audio clips and POSTs them, and the server forwards each to
//! the configured endpoint and stores the returned text as a transcript
//! segment. This is fully self-hostable (run whisper.cpp's server, faster-
//! whisper, LocalAI, ... on localhost), browser-agnostic (capture is just
//! `MediaRecorder`, so Firefox/Safari work), and keeps audio off third-party
//! clouds - at the cost of operator-run STT and CPU.
//!
//! The wire contract is the OpenAI `/v1/audio/transcriptions` shape: a
//! multipart POST with a `file` part + `model` field, responding `{"text": ...}`.
//! Most self-hosted engines expose exactly this.
//!
//! The endpoint is OPERATOR-configured and trusted (same posture as the SMTP /
//! IMAP relays): the request is NOT run through the LC-210 public-IP SSRF
//! filter, precisely so it can reach a `localhost`/internal STT service. Never
//! point `LETS_CHAT_STT_URL` at an untrusted host.
//!
//! LC-483: the same endpoint also transcribes uploaded VOICE MESSAGES (an audio
//! attachment carrying a waveform), off the request path in
//! `routes::room::maybe_transcribe_voice_message`. So configuring STT now sends
//! both call audio AND voice-message audio to the endpoint - relevant to load /
//! cost on a metered engine.

use async_trait::async_trait;

/// Operator STT configuration. `None` from [`SttConfig::from_env`] means server
/// STT is disabled and clients fall back to the browser engine.
#[derive(Debug, Clone)]
pub struct SttConfig {
    /// Full transcription endpoint URL (e.g. `http://127.0.0.1:8090/v1/audio/transcriptions`).
    pub url: String,
    /// Optional bearer token, for engines that require auth.
    pub api_key: Option<String>,
    /// Model name sent in the `model` form field. Defaults to `whisper-1`.
    pub model: String,
}

impl SttConfig {
    /// Read from `LETS_CHAT_STT_URL` (required to enable), `LETS_CHAT_STT_API_KEY`
    /// (optional), `LETS_CHAT_STT_MODEL` (optional, default `whisper-1`).
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("LETS_CHAT_STT_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let api_key = std::env::var("LETS_CHAT_STT_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let model = std::env::var("LETS_CHAT_STT_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "whisper-1".to_string());
        Some(Self {
            url,
            api_key,
            model,
        })
    }
}

#[derive(Debug)]
pub enum SttError {
    /// Network / transport failure reaching the endpoint.
    Transport(String),
    /// The endpoint returned a non-success status or an unparseable body.
    BadResponse(String),
}

impl std::fmt::Display for SttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttError::Transport(e) => write!(f, "stt transport error: {e}"),
            SttError::BadResponse(e) => write!(f, "stt bad response: {e}"),
        }
    }
}

/// One short audio clip -> recognized text (possibly empty). Mockable so the
/// audio route is testable without a live STT service (cf. `PushClient`).
#[async_trait]
pub trait SttClient: Send + Sync {
    async fn transcribe(&self, audio: Vec<u8>, content_type: &str) -> Result<String, SttError>;
}

/// Production client: multipart POST to the operator's OpenAI-compatible
/// endpoint via `http_client::outbound_trusted_post` (the single blessed
/// un-SSRF-filtered path, so a self-hosted localhost engine is reachable; see
/// the module note).
pub struct ReqwestSttClient {
    cfg: SttConfig,
}

impl ReqwestSttClient {
    pub fn new(cfg: SttConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl SttClient for ReqwestSttClient {
    async fn transcribe(&self, audio: Vec<u8>, content_type: &str) -> Result<String, SttError> {
        // Extension matters to some engines that sniff by filename; webm/opus is
        // what MediaRecorder produces by default.
        let filename = if content_type.contains("ogg") {
            "audio.ogg"
        } else if content_type.contains("mp4") || content_type.contains("mpeg") {
            "audio.mp4"
        } else {
            "audio.webm"
        };
        let part = reqwest::multipart::Part::bytes(audio)
            .file_name(filename)
            .mime_str(content_type)
            .map_err(|e| SttError::Transport(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("model", self.cfg.model.clone())
            .part("file", part);
        let mut req = crate::http_client::outbound_trusted_post(&self.cfg.url)
            .await
            .map_err(|e| SttError::Transport(e.to_string()))?
            .multipart(form);
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| SttError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SttError::BadResponse(format!("status {}", resp.status())));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| SttError::BadResponse(e.to_string()))?;
        Ok(body
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .trim()
            .to_string())
    }
}

/// Test double returning a canned transcription.
pub struct MockSttClient {
    pub canned: String,
}

#[async_trait]
impl SttClient for MockSttClient {
    async fn transcribe(&self, _audio: Vec<u8>, _content_type: &str) -> Result<String, SttError> {
        Ok(self.canned.clone())
    }
}
