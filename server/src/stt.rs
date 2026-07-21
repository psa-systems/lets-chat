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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;

/// LC-590: default STT request timeout when `LETS_CHAT_STT_TIMEOUT_SECS` is
/// unset. Deliberately far above the 10s shared `http_client` ceiling: a minute
/// of speech on a CPU-bound self-hosted engine routinely takes tens of seconds,
/// and the shared default silently dropped those clips.
pub const DEFAULT_STT_TIMEOUT_SECS: u64 = 60;

/// LC-590: ceiling for the duration-scaled timeout, matching the recorder's own
/// `MAX_SECONDS = 300` cap (`templates/room/composer.html`). Nothing we submit
/// can be longer than this, so nothing needs longer than base + 300s. An
/// operator who configures a larger base is never clamped below it.
pub const MAX_STT_TIMEOUT_SECS: u64 = 300;

/// LC-590: delay before each retry. The number of attempts is
/// `STT_BACKOFF.len() + 1` (three: immediate, +1s, +4s). Exposed so the retry
/// policy is one visible constant rather than a magic loop bound.
pub const STT_BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(4)];

/// LC-590: the timeout LIVE call clips use instead of the operator's base.
///
/// The base exists for voice messages, which can be minutes long and whose
/// transcript is a permanent artifact worth waiting for. A call clip is 5
/// seconds of audio feeding an ephemeral caption, and the browser posts the next
/// one 5 seconds later regardless. Letting those inherit a 60s base means a
/// HUNG engine (one that accepts the connection and never answers) parks every
/// attempt for the full base: 3 attempts plus backoff is ~3 minutes per clip,
/// with a new clip arriving every 5s, so stalled requests stack up tens deep.
/// The old 10s shared ceiling accidentally bounded that; raising it for voice
/// messages must not un-bound it here.
///
/// A caption that takes longer than this to come back has already scrolled out
/// of usefulness, so there is nothing to lose by cutting it short. Bounding the
/// CONCURRENCY properly (a worker queue) is LC-592's job; this just keeps
/// LC-590 from making the load worse than it found it.
pub const LIVE_CLIP_TIMEOUT_SECS: u64 = 15;

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
    /// LC-590: base request timeout in seconds, replacing the 10s shared
    /// `http_client` default for STT only. Scaled up by the clip's recorded
    /// length where known; see [`stt_timeout`].
    pub timeout_secs: u64,
}

impl SttConfig {
    /// Read from `LETS_CHAT_STT_URL` (required to enable), `LETS_CHAT_STT_API_KEY`
    /// (optional), `LETS_CHAT_STT_MODEL` (optional, default `whisper-1`),
    /// `LETS_CHAT_STT_PROMPT` (optional glossary, LC-591), and
    /// `LETS_CHAT_STT_PROVIDER` (optional, `openai` default; LC-593), and
    /// `LETS_CHAT_STT_TIMEOUT_SECS` (optional, default
    /// [`DEFAULT_STT_TIMEOUT_SECS`]; LC-590). An unrecognized provider value
    /// falls back to `openai`; an unparseable or zero timeout falls back to the
    /// default rather than disabling the timeout.
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
            timeout_secs: var("LETS_CHAT_STT_TIMEOUT_SECS")
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|s| *s > 0)
                .unwrap_or(DEFAULT_STT_TIMEOUT_SECS),
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
    /// Network / transport failure reaching the endpoint, including a timeout.
    Transport(String),
    /// LC-590: the endpoint answered with a non-success HTTP status. Split out
    /// of `BadResponse` so the retry policy can classify it: the status code is
    /// the only signal distinguishing "the engine is briefly unwell" from "this
    /// clip will never be accepted".
    Status(u16),
    /// The endpoint returned a success status with an unparseable body.
    BadResponse(String),
}

impl SttError {
    /// LC-590: whether retrying this failure could plausibly succeed. Transport
    /// failures (connect refused, timeout, reset) and 5xx / 408 / 429 are the
    /// engine being briefly unwell. Every other 4xx is deterministic - a
    /// rejected container, a bad key, a too-large body - and retrying it just
    /// spends the operator's quota to fail three times instead of once.
    pub fn is_transient(&self) -> bool {
        match self {
            SttError::Transport(_) => true,
            SttError::Status(code) => *code >= 500 || *code == 408 || *code == 429,
            SttError::BadResponse(_) => false,
        }
    }
}

impl std::fmt::Display for SttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttError::Transport(e) => write!(f, "stt transport error: {e}"),
            SttError::Status(code) => write!(f, "stt endpoint returned status {code}"),
            SttError::BadResponse(e) => write!(f, "stt bad response: {e}"),
        }
    }
}

/// LC-590: one transcription request. Grew out of the old
/// `(audio, content_type, language)` argument list once the timeout needed the
/// clip's recorded length too; a struct keeps the call sites readable and lets
/// the retry helper clone a request without re-threading four positional args.
#[derive(Debug, Clone)]
pub struct SttRequest {
    pub audio: Vec<u8>,
    pub content_type: String,
    /// Hint (an ISO code like "en"/"es"), or `None` to let the engine
    /// autodetect.
    pub language: Option<String>,
    /// Recorded length in seconds when known, used to scale the timeout. `None`
    /// for a source that carries no duration (then only the base applies).
    pub duration_secs: Option<f32>,
    /// Per-request override of the operator's base timeout. `None` uses
    /// `SttConfig::timeout_secs`.
    pub timeout_secs: Option<u64>,
}

impl SttRequest {
    pub fn new(audio: Vec<u8>, content_type: impl Into<String>) -> Self {
        Self {
            audio,
            content_type: content_type.into(),
            language: None,
            duration_secs: None,
            timeout_secs: None,
        }
    }

    /// Attach a language hint, dropping a blank one so a user with an empty
    /// locale is the same as no hint at all.
    pub fn with_language(mut self, language: Option<&str>) -> Self {
        self.language = language
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string);
        self
    }

    pub fn with_duration_secs(mut self, duration_secs: Option<f32>) -> Self {
        self.duration_secs = duration_secs;
        self
    }

    /// Override the operator's base timeout for this one request. See
    /// [`LIVE_CLIP_TIMEOUT_SECS`] for the only caller and why it exists.
    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }
}

/// LC-590: the effective timeout for one clip. The base is the operator's
/// `LETS_CHAT_STT_TIMEOUT_SECS`; a known recording length is added on top,
/// because a 4-minute voice message legitimately takes far longer to transcribe
/// than a 5-second call clip. Capped at [`MAX_STT_TIMEOUT_SECS`], except that an
/// operator who configures a larger base is never clamped below it.
pub fn stt_timeout(base_secs: u64, duration_secs: Option<f32>) -> Duration {
    let extra = duration_secs
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| d.ceil() as u64)
        .unwrap_or(0);
    let cap = base_secs.max(MAX_STT_TIMEOUT_SECS);
    Duration::from_secs(base_secs.saturating_add(extra).min(cap).max(1))
}

/// One short audio clip -> a recognized [`SttResult`]. Mockable so the audio
/// route is testable without a live STT service (cf. `PushClient`). This is a
/// SINGLE attempt; production callers go through [`transcribe_with_retry`].
#[async_trait]
pub trait SttClient: Send + Sync {
    async fn transcribe(&self, req: SttRequest) -> Result<SttResult, SttError>;
}

/// LC-590: transcribe with the production retry policy ([`STT_BACKOFF`]).
/// Both call paths (call clips and voice messages) use this rather than calling
/// the client directly, so a transient engine hiccup no longer drops a clip on
/// the floor.
pub async fn transcribe_with_retry(
    client: &dyn SttClient,
    req: SttRequest,
) -> Result<SttResult, SttError> {
    transcribe_with_backoff(client, req, &STT_BACKOFF).await
}

/// The retry loop, with the delays injected. Attempts = `backoff.len() + 1`, so
/// an empty slice is exactly the pre-LC-590 single-shot behaviour - which is
/// also what the tests pass, to exercise the policy without sleeping through it.
/// Only transient failures are retried; a permanent one returns immediately.
pub async fn transcribe_with_backoff(
    client: &dyn SttClient,
    req: SttRequest,
    backoff: &[Duration],
) -> Result<SttResult, SttError> {
    let mut attempt = 0usize;
    loop {
        // The clone is one memcpy of a clip (tens of KB for a call clip, a few
        // MB for a long voice message) against a network round-trip, and it is
        // what lets the request be replayed at all. Not worth threading an
        // owned-body-per-attempt through both providers to avoid.
        match client.transcribe(req.clone()).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let Some(delay) = backoff.get(attempt) else {
                    return Err(e);
                };
                if !e.is_transient() {
                    return Err(e);
                }
                tracing::info!(
                    error = %e,
                    attempt = attempt + 1,
                    of = backoff.len() + 1,
                    "stt attempt failed; retrying"
                );
                tokio::time::sleep(*delay).await;
                attempt += 1;
            }
        }
    }
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
    async fn transcribe_openai(&self, req: SttRequest) -> Result<SttResult, SttError> {
        let SttRequest {
            audio,
            content_type,
            language,
            duration_secs,
            timeout_secs,
        } = req;
        let content_type = content_type.as_str();
        let language = language.as_deref();
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
            // LC-590: per-request override of the 10s shared client default.
            .timeout(stt_timeout(
                timeout_secs.unwrap_or(self.cfg.timeout_secs),
                duration_secs,
            ))
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
    async fn transcribe_deepgram(&self, req: SttRequest) -> Result<SttResult, SttError> {
        let SttRequest {
            audio,
            content_type,
            language,
            duration_secs,
            timeout_secs,
        } = req;
        let mut req = crate::http_client::outbound_trusted_post(&self.cfg.url)
            .await
            .map_err(|e| SttError::Transport(e.to_string()))?
            // LC-590: per-request override of the 10s shared client default.
            .timeout(stt_timeout(
                timeout_secs.unwrap_or(self.cfg.timeout_secs),
                duration_secs,
            ))
            .query(&deepgram_query(&self.cfg.model, language.as_deref()))
            .header(reqwest::header::CONTENT_TYPE, content_type)
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
    async fn transcribe(&self, req: SttRequest) -> Result<SttResult, SttError> {
        match self.cfg.provider {
            SttProvider::OpenAi => self.transcribe_openai(req).await,
            SttProvider::Deepgram => self.transcribe_deepgram(req).await,
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
        // LC-590: keep the raw code so `is_transient` can classify it.
        return Err(SttError::Status(resp.status().as_u16()));
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
/// segments" (the plain-json fallback). `fail_first` / `fail_permanent`
/// (LC-590) script failures so the retry policy is testable end to end.
#[derive(Default)]
pub struct MockSttClient {
    pub canned: String,
    pub canned_segments: Vec<SttSegment>,
    /// LC-590: fail this many leading calls before succeeding. `usize::MAX`
    /// means "always fail".
    pub fail_first: usize,
    /// LC-590: make the scripted failures permanent (a 400) instead of
    /// transient, so a test can assert the retry policy does NOT retry them.
    pub fail_permanent: bool,
    /// LC-590: total calls received, so a test can assert the attempt count
    /// rather than only the final outcome.
    pub calls: AtomicUsize,
}

impl MockSttClient {
    /// Construct from just text (no segments) - the common case.
    pub fn text(canned: impl Into<String>) -> Self {
        Self {
            canned: canned.into(),
            ..Default::default()
        }
    }

    /// LC-590: fail the first `n` calls transiently, then return `canned`.
    /// `usize::MAX` never succeeds.
    pub fn failing(canned: impl Into<String>, n: usize) -> Self {
        Self {
            canned: canned.into(),
            fail_first: n,
            ..Default::default()
        }
    }

    /// LC-590: fail every call with a permanent (non-retryable) 400.
    pub fn failing_permanently() -> Self {
        Self {
            fail_first: usize::MAX,
            fail_permanent: true,
            ..Default::default()
        }
    }

    /// Number of `transcribe` calls received so far.
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SttClient for MockSttClient {
    async fn transcribe(&self, _req: SttRequest) -> Result<SttResult, SttError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_first {
            return Err(if self.fail_permanent {
                SttError::Status(400)
            } else {
                SttError::Transport("mock transient failure".into())
            });
        }
        Ok(SttResult {
            text: self.canned.clone(),
            segments: self.canned_segments.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `from_env` tests mutate process-global environment variables, which
    /// the test harness would otherwise interleave across threads (one test
    /// removing `LETS_CHAT_STT_URL` while another is mid-read). Every env test
    /// holds this. Sync `Mutex` is fine: none of them await.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    fn timeout_scales_with_clip_length_and_is_capped() {
        // No known duration -> the operator's base, unchanged.
        assert_eq!(stt_timeout(60, None), Duration::from_secs(60));
        // A known duration is added on top, rounded up to the second.
        assert_eq!(stt_timeout(60, Some(12.2)), Duration::from_secs(73));
        // Capped at the recorder's 300s ceiling.
        assert_eq!(stt_timeout(60, Some(600.0)), Duration::from_secs(300));
        // An operator who configures a larger base is never clamped BELOW it,
        // which a naive `.min(300)` would do.
        assert_eq!(stt_timeout(900, None), Duration::from_secs(900));
        // Garbage durations degrade to "unknown", never to a zero timeout.
        assert_eq!(stt_timeout(60, Some(f32::NAN)), Duration::from_secs(60));
        assert_eq!(stt_timeout(60, Some(-5.0)), Duration::from_secs(60));
    }

    #[test]
    fn live_clips_override_the_operator_base() {
        // The live path must not inherit a large operator base: a hung engine
        // would then park three attempts per 5-second clip while the browser
        // keeps posting new ones. The override is what bounds that.
        let req = SttRequest::new(vec![], "audio/webm").with_timeout_secs(LIVE_CLIP_TIMEOUT_SECS);
        assert_eq!(req.timeout_secs, Some(15));
        // The override only earns its keep by being shorter than the base, and
        // shorter than the 10s-era behaviour is not required - just bounded.
        assert_eq!(
            stt_timeout(LIVE_CLIP_TIMEOUT_SECS, None),
            Duration::from_secs(15)
        );
        // Voice messages leave it unset and get the operator's base.
        assert_eq!(SttRequest::new(vec![], "audio/webm").timeout_secs, None);
    }

    #[test]
    fn transient_classification_drives_retry() {
        assert!(SttError::Transport("connect refused".into()).is_transient());
        assert!(SttError::Status(503).is_transient());
        assert!(SttError::Status(429).is_transient(), "rate limit backs off");
        assert!(SttError::Status(408).is_transient());
        assert!(!SttError::Status(400).is_transient(), "bad clip is final");
        assert!(!SttError::Status(401).is_transient(), "bad key is final");
        assert!(!SttError::BadResponse("bad json".into()).is_transient());
    }

    #[tokio::test]
    async fn retries_transient_failures_then_succeeds() {
        // Two transient failures then success: the default policy allows three
        // attempts, so this recovers. Zero delays keep the test instant.
        let mock = MockSttClient::failing("recovered", 2);
        let zero = [Duration::ZERO, Duration::ZERO];
        let r = transcribe_with_backoff(&mock, SttRequest::new(vec![1], "audio/webm"), &zero)
            .await
            .expect("third attempt succeeds");
        assert_eq!(r.text, "recovered");
        assert_eq!(mock.call_count(), 3, "two retries after the first attempt");
    }

    #[tokio::test]
    async fn gives_up_after_the_configured_attempts() {
        let mock = MockSttClient::failing("never", usize::MAX);
        let zero = [Duration::ZERO, Duration::ZERO];
        let err = transcribe_with_backoff(&mock, SttRequest::new(vec![1], "audio/webm"), &zero)
            .await
            .expect_err("exhausted");
        assert!(
            err.is_transient(),
            "the last error is still the transient one"
        );
        assert_eq!(mock.call_count(), 3, "bounded, not infinite");
    }

    #[tokio::test]
    async fn permanent_failures_are_not_retried() {
        let mock = MockSttClient::failing_permanently();
        let zero = [Duration::ZERO, Duration::ZERO];
        let err = transcribe_with_backoff(&mock, SttRequest::new(vec![1], "audio/webm"), &zero)
            .await
            .expect_err("4xx is final");
        assert!(matches!(err, SttError::Status(400)));
        assert_eq!(
            mock.call_count(),
            1,
            "a deterministic 4xx must not burn two more calls"
        );
    }

    #[test]
    fn production_policy_is_three_attempts() {
        // The prod path injects STT_BACKOFF; the tests above inject zeros, so
        // pin the real policy here rather than leaving it unasserted.
        assert_eq!(STT_BACKOFF.len() + 1, 3, "three attempts");
        assert_eq!(STT_BACKOFF[0], Duration::from_secs(1));
        assert_eq!(STT_BACKOFF[1], Duration::from_secs(4));
    }

    #[test]
    fn timeout_from_env_falls_back_on_garbage() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded test; vars removed at the end.
        unsafe {
            std::env::set_var("LETS_CHAT_STT_URL", "http://127.0.0.1:9/v1/x");
        }
        assert_eq!(
            SttConfig::from_env().unwrap().timeout_secs,
            DEFAULT_STT_TIMEOUT_SECS
        );
        unsafe { std::env::set_var("LETS_CHAT_STT_TIMEOUT_SECS", "120") };
        assert_eq!(SttConfig::from_env().unwrap().timeout_secs, 120);
        // Unparseable and zero both fall back rather than disabling the timeout.
        unsafe { std::env::set_var("LETS_CHAT_STT_TIMEOUT_SECS", "soon") };
        assert_eq!(
            SttConfig::from_env().unwrap().timeout_secs,
            DEFAULT_STT_TIMEOUT_SECS
        );
        unsafe { std::env::set_var("LETS_CHAT_STT_TIMEOUT_SECS", "0") };
        assert_eq!(
            SttConfig::from_env().unwrap().timeout_secs,
            DEFAULT_STT_TIMEOUT_SECS,
            "0 would mean no timeout at all; fall back instead"
        );
        unsafe {
            std::env::remove_var("LETS_CHAT_STT_URL");
            std::env::remove_var("LETS_CHAT_STT_TIMEOUT_SECS");
        }
    }

    #[test]
    fn malformed_deepgram_body_is_empty_not_error() {
        let r = parse_deepgram_result(&serde_json::json!({ "results": {} }));
        assert_eq!(r.text, "");
        assert!(r.segments.is_empty());
    }
}
