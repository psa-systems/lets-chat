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

/// LC-593: which provider's wire shape the STT endpoint speaks. Selects the
/// request builder and response parser behind [`SttClient`]; the normalized
/// [`SttResult`] is identical across providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SttProvider {
    /// OpenAI `/v1/audio/transcriptions`: multipart `file` + `model`, JSON
    /// `{text, segments}`. The default, and what most self-hosted engines
    /// (whisper.cpp, faster-whisper, LocalAI, ...) expose.
    #[default]
    OpenAi,
    /// Deepgram prerecorded `/v1/listen`: raw audio body, `Authorization: Token`,
    /// JSON `results.channels[0].alternatives[0]` (transcript + word timings).
    Deepgram,
}

impl SttProvider {
    fn parse_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAi),
            "deepgram" => Some(Self::Deepgram),
            _ => None,
        }
    }
}

/// Operator STT configuration. `None` from [`SttConfig::from_env`] means server
/// STT is disabled and clients fall back to the browser engine.
#[derive(Debug, Clone)]
pub struct SttConfig {
    /// LC-593: the provider wire shape (`openai` default, or `deepgram`).
    pub provider: SttProvider,
    /// Full transcription endpoint URL (e.g. `http://127.0.0.1:8090/v1/audio/transcriptions`
    /// for OpenAI, `https://api.deepgram.com/v1/listen` for Deepgram).
    pub url: String,
    /// Optional bearer/token key, for engines that require auth.
    pub api_key: Option<String>,
    /// Model name. Sent as the `model` form field (OpenAI) or `model` query
    /// param (Deepgram). Defaults to `whisper-1`.
    pub model: String,
    /// LC-591: optional operator glossary/style hint. Sent as the `prompt` field
    /// (OpenAI); Deepgram has no equivalent and ignores it. A single global
    /// string; per-room glossaries are out of scope.
    pub prompt: Option<String>,
}

impl SttConfig {
    /// Read from `LETS_CHAT_STT_URL` (required to enable), `LETS_CHAT_STT_API_KEY`
    /// (optional), `LETS_CHAT_STT_MODEL` (optional, default `whisper-1`),
    /// `LETS_CHAT_STT_PROMPT` (optional glossary, LC-591), and
    /// `LETS_CHAT_STT_PROVIDER` (optional, `openai` default; LC-593). An
    /// unrecognized provider value falls back to `openai`.
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let url = var("LETS_CHAT_STT_URL")?;
        let provider = var("LETS_CHAT_STT_PROVIDER")
            .and_then(|p| SttProvider::parse_str(&p))
            .unwrap_or_default();
        Some(Self {
            provider,
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

impl ReqwestSttClient {
    /// OpenAI `/v1/audio/transcriptions`: multipart `file` + `model`, asking for
    /// `verbose_json` segment timings, with the language + prompt hints (LC-591).
    async fn transcribe_openai(
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
        let body = send_for_json(req).await?;
        Ok(parse_openai_result(&body))
    }

    /// LC-593: Deepgram prerecorded `/v1/listen`: the raw audio as the request
    /// body (not multipart), `Authorization: Token <key>`, and model/language
    /// as query params. Deepgram has no `prompt` equivalent, so the operator
    /// glossary does not apply here.
    async fn transcribe_deepgram(
        &self,
        audio: Vec<u8>,
        content_type: &str,
        language: Option<&str>,
    ) -> Result<SttResult, SttError> {
        let mut req = crate::http_client::outbound_trusted_post(&self.cfg.url)
            .await
            .map_err(|e| SttError::Transport(e.to_string()))?
            .query(&deepgram_query(&self.cfg.model, language))
            .header(reqwest::header::CONTENT_TYPE, content_type.to_string())
            .body(audio);
        if let Some(key) = &self.cfg.api_key {
            // Deepgram uses the `Token` auth scheme, not `Bearer`.
            req = req.header(reqwest::header::AUTHORIZATION, format!("Token {key}"));
        }
        let body = send_for_json(req).await?;
        Ok(parse_deepgram_result(&body))
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
        match self.cfg.provider {
            SttProvider::OpenAi => self.transcribe_openai(audio, content_type, language).await,
            SttProvider::Deepgram => {
                self.transcribe_deepgram(audio, content_type, language)
                    .await
            }
        }
    }
}

/// Send a prepared request and parse a success JSON body, mapping transport and
/// non-2xx failures to [`SttError`]. Shared by both providers.
async fn send_for_json(req: reqwest::RequestBuilder) -> Result<serde_json::Value, SttError> {
    let resp = req
        .send()
        .await
        .map_err(|e| SttError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(SttError::BadResponse(format!("status {}", resp.status())));
    }
    resp.json()
        .await
        .map_err(|e| SttError::BadResponse(e.to_string()))
}

/// LC-593: the Deepgram query parameters. `model` always; `language` when a
/// hint is given (else Deepgram autodetects); `punctuate` + `smart_format` for
/// readable output. Split out so the request shape is unit-testable without a
/// live endpoint.
pub fn deepgram_query(model: &str, language: Option<&str>) -> Vec<(String, String)> {
    let mut params = vec![
        ("model".to_string(), model.to_string()),
        ("punctuate".to_string(), "true".to_string()),
        ("smart_format".to_string(), "true".to_string()),
    ];
    if let Some(lang) = language.filter(|l| !l.trim().is_empty()) {
        params.push(("language".to_string(), lang.trim().to_string()));
    }
    params
}

/// LC-593: parse a Deepgram prerecorded response into the normalized
/// [`SttResult`]. `text` is the first channel's first alternative transcript;
/// the alternative's `words[]` give the real timings, aggregated into one
/// clip-level segment (`start` = first word, `end` = last word) - which is what
/// the downstream duration/VTT path consumes. A body missing the transcript
/// yields an empty result, matching the OpenAI path's tolerance.
pub fn parse_deepgram_result(body: &serde_json::Value) -> SttResult {
    let alt = body
        .get("results")
        .and_then(|r| r.get("channels"))
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|ch| ch.get("alternatives"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first());
    let Some(alt) = alt else {
        return SttResult::default();
    };
    let text = alt
        .get("transcript")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let words = alt.get("words").and_then(|w| w.as_array());
    let segments = match words {
        Some(words) if !words.is_empty() && !text.is_empty() => {
            let start = words
                .first()
                .and_then(|w| w.get("start"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let end = words
                .last()
                .and_then(|w| w.get("end"))
                .and_then(|v| v.as_f64())
                .unwrap_or(start);
            vec![SttSegment {
                start,
                end,
                text: text.clone(),
            }]
        }
        _ => Vec::new(),
    };
    SttResult { text, segments }
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
        assert_eq!(
            cfg.provider,
            SttProvider::OpenAi,
            "provider defaults to openai"
        );
        unsafe {
            std::env::remove_var("LETS_CHAT_STT_URL");
            std::env::remove_var("LETS_CHAT_STT_PROMPT");
        }
    }

    #[test]
    fn provider_selection_from_env_defaults_to_openai() {
        assert_eq!(
            SttProvider::parse_str("deepgram"),
            Some(SttProvider::Deepgram)
        );
        assert_eq!(
            SttProvider::parse_str("  OpenAI "),
            Some(SttProvider::OpenAi)
        );
        assert_eq!(SttProvider::parse_str("whisper"), None, "unknown -> None");

        // SAFETY: single-threaded test; vars removed at the end.
        unsafe {
            std::env::set_var("LETS_CHAT_STT_URL", "https://api.deepgram.com/v1/listen");
            std::env::set_var("LETS_CHAT_STT_PROVIDER", "deepgram");
        }
        assert_eq!(
            SttConfig::from_env().unwrap().provider,
            SttProvider::Deepgram
        );
        // An unrecognized value falls back to openai, never an error.
        unsafe { std::env::set_var("LETS_CHAT_STT_PROVIDER", "bogus") };
        assert_eq!(SttConfig::from_env().unwrap().provider, SttProvider::OpenAi);
        unsafe {
            std::env::remove_var("LETS_CHAT_STT_URL");
            std::env::remove_var("LETS_CHAT_STT_PROVIDER");
        }
    }

    #[test]
    fn deepgram_query_shape() {
        let q = deepgram_query("nova-2", Some(" es "));
        assert!(q.contains(&("model".into(), "nova-2".into())));
        assert!(q.contains(&("punctuate".into(), "true".into())));
        assert!(q.contains(&("smart_format".into(), "true".into())));
        assert!(
            q.contains(&("language".into(), "es".into())),
            "language hint is trimmed and included"
        );
        // No language hint -> no language param (Deepgram autodetects).
        let q = deepgram_query("nova-2", None);
        assert!(!q.iter().any(|(k, _)| k == "language"));
    }

    #[test]
    fn parses_deepgram_response_with_word_timings() {
        let body = serde_json::json!({
            "results": { "channels": [ { "alternatives": [ {
                "transcript": "hello there friend",
                "words": [
                    { "word": "hello", "start": 0.1, "end": 0.5 },
                    { "word": "there", "start": 0.5, "end": 0.9 },
                    { "word": "friend", "start": 1.0, "end": 1.6 },
                ],
            } ] } ] },
        });
        let r = parse_deepgram_result(&body);
        assert_eq!(r.text, "hello there friend");
        assert_eq!(r.segments.len(), 1, "words aggregate to one clip segment");
        assert_eq!(r.segments[0].start, 0.1);
        assert_eq!(r.segments[0].end, 1.6);
        // duration = 1.6 - 0.1 = 1.5s.
        assert_eq!(r.duration_ms(), 1500);
    }

    #[test]
    fn deepgram_response_without_words_has_no_segments() {
        let body = serde_json::json!({
            "results": { "channels": [ { "alternatives": [ {
                "transcript": "  no timings here  "
            } ] } ] },
        });
        let r = parse_deepgram_result(&body);
        assert_eq!(r.text, "no timings here", "transcript is trimmed");
        assert!(
            r.segments.is_empty(),
            "no words -> no segments (synthetic fallback)"
        );
        assert_eq!(r.duration_ms(), 0);
    }

    #[test]
    fn malformed_deepgram_body_is_empty_not_error() {
        let r = parse_deepgram_result(&serde_json::json!({ "results": {} }));
        assert_eq!(r.text, "");
        assert!(r.segments.is_empty());
    }
}
