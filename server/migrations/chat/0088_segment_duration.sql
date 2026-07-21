-- LC-591: real spoken duration of a transcript segment, in milliseconds, from
-- the STT engine's verbose_json segment timings. 0 = unknown (the browser
-- Web Speech path, or an engine that ignored response_format=verbose_json), in
-- which case the WebVTT export falls back to its pre-LC-591 synthetic cue length.
ALTER TABLE transcript_segments ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
