-- LC-665: per-room opt-in for the scheduled AI activity digest, plus the
-- last-run timestamp used to dedupe (post at most once per interval). Off by
-- default; a room manager turns it on, and it only runs when the operator has
-- configured an LLM endpoint (LETS_CHAT_LLM_URL). Distinct from the email
-- digest (crate::digest): this posts an AI recap in-channel as the assistant
-- bot. Columns on rooms, read via dedicated getters like assistant_enabled.
ALTER TABLE rooms ADD COLUMN digest_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE rooms ADD COLUMN digest_last_at TEXT;
