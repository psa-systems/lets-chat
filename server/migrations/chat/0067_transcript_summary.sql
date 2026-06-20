-- LC-396: optional AI summary / action-items for a saved call transcript.
-- Generated on demand from an operator-configured LLM endpoint (LETS_CHAT_LLM_*)
-- and cached here so it is computed once. NULL = not yet summarized. Stored as
-- markdown (the LLM returns a summary + bulleted action items), rendered through
-- the same markdown pipeline as messages; length-capped on the write path.
ALTER TABLE call_transcripts ADD COLUMN summary TEXT;
