-- LC-483: server-side transcription of voice messages.
-- When LETS_CHAT_STT_* is configured, an uploaded voice message (an audio
-- attachment carrying a waveform) is transcribed off the request path via the
-- same OpenAI-compatible STT endpoint used for call transcription, and the
-- recognized text is stored here. NULL means "not transcribed" (STT disabled,
-- still in flight, transcription failed, or the attachment is not a voice
-- message). Rendered as plain (HTML-escaped) text under the waveform player.
ALTER TABLE file_uploads ADD COLUMN transcript TEXT;
