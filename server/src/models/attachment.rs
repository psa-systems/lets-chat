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
    /// LC-590: outcome of the last transcription attempt (`pending` / `done` /
    /// `failed`), or `None` when none was ever attempted. Drives the "queued"
    /// and "failed, retry" states under the player; a `done` status is implicit
    /// in `transcript` being present, so the template does not read it.
    pub transcript_status: Option<String>,
    /// LC-537: author-provided alt text for accessibility. `None` when the
    /// uploader has not set one; the renderer then falls back to the filename.
    pub alt_text: Option<String>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    /// LC-537: effective `alt` attribute for an image: the author's alt text
    /// when set and non-blank, otherwise the filename (v1 behaviour).
    pub fn alt(&self) -> &str {
        match self.alt_text.as_deref() {
            Some(t) if !t.trim().is_empty() => t,
            _ => &self.filename,
        }
    }

    /// LC-537: whether the author has set explicit alt text (drives the "edit"
    /// vs "add" label on the alt-text control).
    pub fn has_alt(&self) -> bool {
        self.alt_text
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    }

    pub fn is_audio(&self) -> bool {
        self.mime_type.starts_with("audio/")
    }

    /// LC-496: a recorded video clip (camera or screen). Rendered as an inline
    /// `<video>` player; its audio is server-transcribed like a voice message,
    /// so the same `transcript` field carries the result.
    pub fn is_video(&self) -> bool {
        self.mime_type.starts_with("video/")
    }

    /// LC-507: a GIF (picker-posted or user-uploaded). The timeline serves the
    /// animated original for these rather than the still first-frame preview the
    /// pipeline produces, so posted GIFs autoplay inline (Slack/Discord-style)
    /// instead of looking frozen until clicked.
    pub fn is_gif(&self) -> bool {
        self.mime_type == "image/gif"
    }

    /// LC-500: human-readable size (B / KB / MB) for the download card. Mirrors
    /// the composer's client-side `humanBytes` so timeline + composer agree.
    pub fn human_size(&self) -> String {
        let n = self.size_bytes;
        if n < 1024 {
            format!("{n} B")
        } else if n < 1024 * 1024 {
            format!("{:.1} KB", n as f64 / 1024.0)
        } else {
            format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
        }
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

    /// LC-590: the last transcription attempt exhausted its retries. The
    /// template renders a Retry control instead of leaving the failure invisible.
    pub fn transcript_failed(&self) -> bool {
        self.transcript_status.as_deref() == Some("failed")
    }

    /// LC-590: a retry has been queued and is running off the request path.
    pub fn transcript_pending(&self) -> bool {
        self.transcript_status.as_deref() == Some("pending")
    }

    /// Recording duration for the template's `data-duration` attribute.
    /// Empty string when unknown; the player then probes the audio element.
    pub fn duration_secs(&self) -> String {
        match self.voice_duration {
            Some(d) => format!("{d:.3}"),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Attachment;

    fn img(alt: Option<&str>) -> Attachment {
        Attachment {
            id: 1,
            filename: "cat.png".into(),
            mime_type: "image/png".into(),
            size_bytes: 10,
            url: "/api/files/1".into(),
            waveform: None,
            voice_duration: None,
            transcript: None,
            transcript_status: None,
            alt_text: alt.map(str::to_string),
        }
    }

    #[test]
    fn alt_prefers_alt_text_then_falls_back_to_filename() {
        assert_eq!(img(Some("a napping cat")).alt(), "a napping cat");
        assert_eq!(img(None).alt(), "cat.png");
        // A blank / whitespace alt is treated as unset (fallback to filename).
        assert_eq!(img(Some("   ")).alt(), "cat.png");
    }

    #[test]
    fn has_alt_true_only_for_non_blank() {
        assert!(img(Some("desc")).has_alt());
        assert!(!img(None).has_alt());
        assert!(!img(Some("  ")).has_alt());
    }
}
