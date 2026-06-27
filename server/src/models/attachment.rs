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
    /// Normalized waveform peaks (0..1) for voice messages, computed
    /// client-side at record time. `None` for every non-voice attachment.
    pub waveform: Option<Vec<f32>>,
    /// Recording duration in seconds for voice messages, captured client-side
    /// at record time. `None` for non-voice attachments (or voice messages
    /// whose client could not determine the duration).
    pub voice_duration: Option<f32>,
    /// LC-483: server-side transcript of a voice message, produced off the
    /// request path via the configured STT endpoint. `None` until (and unless)
    /// transcription completes; always `None` when STT is disabled or the
    /// attachment is not a voice message. Rendered as escaped text, so it is
    /// safe to render directly.
    pub transcript: Option<String>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    pub fn is_audio(&self) -> bool {
        self.mime_type.starts_with("audio/")
    }

    /// Waveform peaks rendered as a comma-separated string for the template's
    /// `data-waveform` attribute. Empty string when there is no waveform.
    pub fn waveform_csv(&self) -> String {
        match &self.waveform {
            Some(peaks) => peaks
                .iter()
                .map(|p| format!("{p:.3}"))
                .collect::<Vec<_>>()
                .join(","),
            None => String::new(),
        }
    }

    /// Recording duration for the template's `data-duration` attribute.
    /// Empty string when unknown; the player then probes the audio element.
    pub fn duration_attr(&self) -> String {
        match self.voice_duration {
            Some(d) => format!("{d:.3}"),
            None => String::new(),
        }
    }
}
