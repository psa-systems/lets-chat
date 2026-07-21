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
    /// LC-591: optional operator glossary/style hint sent as the `prompt` field
    /// (whisper uses it to bias spelling of names and jargon). A single global
    /// string; per-room glossaries are out of scope.
    pub prompt: Option<String>,
}

impl SttConfig {
    /// Read from `LETS_CHAT_STT_URL` (required to enable), `LETS_CHAT_STT_API_KEY`
    /// (optional), `LETS_CHAT_STT_MODEL` (optional, default `whisper-1`),
    /// `LETS_CHAT_STT_PROMPT` (optional glossary, LC-591).
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let url = var("LETS_CHAT_STT_URL")?;
        Some(Self {
            url,
            api_key: var("LETS_CHAT_STT_API_KEY"),
            model: var("LETS_CHAT_STT_MODEL").unwrap_or_else(|| "whisper-1".to_string()),
            prompt: var("LETS_CHAT_STT_PROMPT"),
        })
    }
}

/// LC-591: one timed piece of a transcription, in seconds relative to the start
/// of the submitted clip. Present only when the engine honours
/// `response_format=verbose_json`; an engine that ignores it yields a result
/// with `text` and no segments.
#[derive(Debug, Clone, PartialEq)]
pub struct SttSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// LC-591: the normalized result of transcribing one clip. `text` is the full
/// recognized string (kept for voice-note display and as the caption body);
/// `segments` carries real timings when the engine provides them, else empty.
/// Every provider (LC-593) normalizes onto this shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SttResult {
    pub text: String,
    pub segments: Vec<SttSegment>,
}

impl SttResult {
    /// The clip's spoken span in milliseconds: last segment end minus first
    /// segment start. 0 when the engine returned no segments, so callers fall
    /// back to the pre-LC-591 synthetic cue duration.
    pub fn duration_ms(&self) -> i64 {
        match (self.segments.first(), self.segments.last()) {
            (Some(f), Some(l)) => (((l.end - f.start) * 1000.0).round() as i64).max(0),
            _ => 0,
        }
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

/// One short audio clip -> a recognized [`SttResult`]. `language` is a hint (an
/// ISO code like "en"/"es", or `None` to let the engine autodetect). Mockable
/// so the audio route is testable without a live STT service (cf. `PushClient`).
#[async_trait]
pub trait SttClient: Send + Sync {
    async fn transcribe(
        &self,
        audio: Vec<u8>,
        content_type: &str,
        language: Option<&str>,
    ) -> Result<SttResult, SttError>;
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
    async fn transcribe(
        &self,
        audio: Vec<u8>,
        content_type: &str,
        language: Option<&str>,
    ) -> Result<SttResult, SttError> {
        // Extension matters to some engines that sniff by filename; webm/opus is
        // what MediaRecorder produces by default.
        // LC-496: clips are video containers (video/webm|mp4|quicktime); voice
        // messages are audio containers. Either way the filename extension must
        // match the container so engines that sniff by name route it correctly.
        let filename = if content_type.contains("quicktime") || content_type.contains("mov") {
            "clip.mov"
        } else if content_type.contains("ogg") {
            "audio.ogg"
        } else if content_type.contains("mp4") || content_type.contains("mpeg") {
            "media.mp4"
        } else {
            "media.webm"
        };
        let part = reqwest::multipart::Part::bytes(audio)
            .file_name(filename)
            .mime_str(content_type)
            .map_err(|e| SttError::Transport(e.to_string()))?;
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.cfg.model.clone())
            // LC-591: ask for segment timings. An engine that does not support
            // verbose_json ignores this field and returns the plain {text}
            // shape, which `parse_openai_result` still handles.
            .text("response_format", "verbose_json")
            .part("file", part);
        // LC-591: language hint when known (else the engine autodetects), and
        // the operator glossary when configured.
        if let Some(lang) = language.filter(|l| !l.trim().is_empty()) {
            form = form.text("language", lang.trim().to_string());
        }
        if let Some(prompt) = self.cfg.prompt.clone() {
            form = form.text("prompt", prompt);
        }
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
        Ok(parse_openai_result(&body))
    }
}

/// LC-591: parse the OpenAI-compatible transcription body into an [`SttResult`],
/// tolerating both the verbose_json shape (`text` + `segments[]{start,end,text}`)
/// and the plain shape (`{text}` only, no segments). A malformed/missing `text`
/// yields an empty string, matching the pre-LC-591 behaviour. Split out so it is
/// unit-testable without a live endpoint. Reused by the OpenAI provider (LC-593).
pub fn parse_openai_result(body: &serde_json::Value) -> SttResult {
    let text = body
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let segments = body
        .get("segments")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let t = s.get("text").and_then(|t| t.as_str())?.trim().to_string();
                    Some(SttSegment {
                        start: s.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        end: s.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        text: t,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    SttResult { text, segments }
}

/// Test double returning a canned transcription. `canned_segments` (LC-591) lets
/// a test exercise the real-timestamp path; empty means "engine returned no
/// segments" (the plain-json fallback).
#[derive(Default)]
pub struct MockSttClient {
    pub canned: String,
    pub canned_segments: Vec<SttSegment>,
}

impl MockSttClient {
    /// Construct from just text (no segments) - the common case.
    pub fn text(canned: impl Into<String>) -> Self {
        Self {
            canned: canned.into(),
            canned_segments: Vec::new(),
        }
    }
}

#[async_trait]
impl SttClient for MockSttClient {
    async fn transcribe(
        &self,
        _audio: Vec<u8>,
        _content_type: &str,
        _language: Option<&str>,
    ) -> Result<SttResult, SttError> {
        Ok(SttResult {
            text: self.canned.clone(),
            segments: self.canned_segments.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbose_json_with_segments() {
        let body = serde_json::json!({
            "text": "hello world how are you",
            "segments": [
                { "start": 0.0, "end": 1.2, "text": "hello world" },
                { "start": 1.5, "end": 3.0, "text": " how are you " },
            ],
        });
        let r = parse_openai_result(&body);
        assert_eq!(r.text, "hello world how are you");
        assert_eq!(r.segments.len(), 2);
        assert_eq!(r.segments[0].start, 0.0);
        assert_eq!(r.segments[0].end, 1.2);
        assert_eq!(r.segments[1].text, "how are you", "segment text is trimmed");
        // duration = last.end - first.start = 3.0 - 0.0 = 3000ms.
        assert_eq!(r.duration_ms(), 3000);
    }

    #[test]
    fn falls_back_to_plain_text_when_no_segments() {
        // An engine that ignores response_format returns the plain shape.
        let body = serde_json::json!({ "text": "  just text  " });
        let r = parse_openai_result(&body);
        assert_eq!(r.text, "just text", "text is trimmed");
        assert!(r.segments.is_empty(), "no segments -> empty, not an error");
        assert_eq!(
            r.duration_ms(),
            0,
            "no segments -> 0 so the export stays synthetic"
        );
    }

    #[test]
    fn malformed_body_yields_empty_text() {
        // Matches the pre-LC-591 behaviour: a body with no usable text is empty,
        // not an error (the caller drops empty results).
        let r = parse_openai_result(&serde_json::json!({ "unexpected": 1 }));
        assert_eq!(r.text, "");
        assert!(r.segments.is_empty());
    }

    #[test]
    fn from_env_reads_prompt() {
        // SAFETY: single-threaded test; vars removed at the end.
        unsafe {
            std::env::set_var(
                "LETS_CHAT_STT_URL",
                "http://127.0.0.1:9/v1/audio/transcriptions",
            );
            std::env::set_var("LETS_CHAT_STT_PROMPT", "  Acme, Foo Bar  ");
        }
        let cfg = SttConfig::from_env().expect("configured");
        assert_eq!(
            cfg.prompt.as_deref(),
            Some("Acme, Foo Bar"),
            "prompt is trimmed"
        );
        assert_eq!(cfg.model, "whisper-1", "default model unchanged");
        unsafe {
            std::env::remove_var("LETS_CHAT_STT_URL");
            std::env::remove_var("LETS_CHAT_STT_PROMPT");
        }
    }
}
